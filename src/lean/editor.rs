//! Observe native LSP evidence; never accept a browser's claim that a proof passed.
use super::Input;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(untagged)]
pub(crate) enum RpcId {
    Number(i64),
    Text(String),
}
#[derive(Deserialize)]
pub(crate) struct Message {
    pub method: Option<String>,
    pub id: Option<RpcId>,
}
enum Pending {
    Initialize,
    Diagnostics(String, u64),
    Imports(String, u64),
}
pub(crate) struct Document {
    pub source: String,
    pub version: u64,
    environment: String,
    complete: Option<u64>,
    diagnostics: Vec<Value>,
    imports: Option<Vec<Value>>,
}
#[derive(Default)]
pub(crate) struct Observer {
    prepared: HashMap<String, Input>,
    workspace_keys: HashMap<String, String>,
    documents: HashMap<String, Document>,
    pending: HashMap<RpcId, Pending>,
}
impl Observer {
    pub fn prepare(&mut self, uri: String, input: Input) {
        self.workspace_keys.extend(
            input
                .chain
                .iter()
                .map(|(n, k)| (n.fnode.clone(), k.clone())),
        );
        self.prepared.insert(uri, input);
    }
    pub fn document(&self, uri: &str, input: &Input) -> Result<&Document> {
        let doc = self
            .documents
            .get(uri)
            .context("Lean document is not open yet")?;
        let node = &input.chain.last().context("no Lean target")?.0;
        if node.source("lean") != Some(doc.source.as_str()) {
            bail!("Lean draft differs from the saved source");
        }
        if doc.environment != input.environment_key()? {
            bail!("Lean dependencies changed; reload the environment");
        }
        // Other open documents can refresh this workspace's sources. Require all
        // dependency versions to still belong to the environment we observed.
        for (node, key) in &input.chain[..input.chain.len() - 1] {
            if self.workspace_keys.get(&node.fnode) != Some(key) {
                bail!("Lean dependencies changed; reload the environment");
            }
        }
        Ok(doc)
    }
    pub fn evidence(
        &self,
        uri: &str,
        input: &Input,
        version: u64,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        let doc = self.document(uri, input)?;
        if doc.version != version || doc.complete != Some(version) {
            bail!("Lean diagnostics are not complete for this version");
        }
        Ok((
            doc.diagnostics.clone(),
            doc.imports
                .clone()
                .context("Lean module imports are not ready")?,
        ))
    }
    pub fn client(&mut self, text: &str, meta: &Message) -> Result<()> {
        let method = meta.method.as_deref().unwrap_or("");
        if !matches!(
            method,
            "initialize"
                | "textDocument/didOpen"
                | "textDocument/didChange"
                | "textDocument/didClose"
                | "textDocument/waitForDiagnostics"
                | "$/lean/moduleHierarchy/imports"
        ) {
            return Ok(());
        }
        let value: Value = serde_json::from_str(text)?;
        let p = &value["params"];
        let uri = p["textDocument"]["uri"].as_str().unwrap_or("");
        let pending = match method {
            "initialize" => Some(Pending::Initialize),
            "textDocument/didOpen" => {
                let input = self
                    .prepared
                    .get(uri)
                    .context("editor may only open selected draft modules")?;
                self.documents.insert(
                    uri.into(),
                    Document {
                        source: p["textDocument"]["text"]
                            .as_str()
                            .context("Lean source missing")?
                            .into(),
                        version: p["textDocument"]["version"]
                            .as_u64()
                            .context("Lean version missing")?,
                        environment: input.environment_key()?,
                        complete: None,
                        diagnostics: vec![],
                        imports: None,
                    },
                );
                None
            }
            "textDocument/didChange" => {
                let doc = self
                    .documents
                    .get_mut(uri)
                    .context("Lean document is not open")?;
                let version = p["textDocument"]["version"]
                    .as_u64()
                    .context("Lean version missing")?;
                if version <= doc.version {
                    bail!("Lean document version must increase");
                }
                for change in p["contentChanges"]
                    .as_array()
                    .context("Lean changes missing")?
                {
                    if change.get("range").is_some() {
                        bail!("editor must use the advertised full document synchronization");
                    }
                    doc.source = change["text"]
                        .as_str()
                        .context("Lean source missing")?
                        .into();
                }
                doc.version = version;
                doc.complete = None;
                doc.imports = None;
                doc.diagnostics.clear();
                None
            }
            "textDocument/didClose" => {
                self.documents.remove(uri);
                None
            }
            "textDocument/waitForDiagnostics" => Some(Pending::Diagnostics(
                p["uri"].as_str().context("Lean URI missing")?.into(),
                p["version"].as_u64().context("Lean version missing")?,
            )),
            "$/lean/moduleHierarchy/imports" => {
                let uri = p["module"]["uri"].as_str().unwrap_or("");
                self.documents
                    .get(uri)
                    .filter(|_| {
                        self.prepared.get(uri).is_some_and(|i| {
                            p["module"]["name"].as_str()
                                == Some(i.chain.last().unwrap().0.module.as_str())
                        })
                    })
                    .map(|doc| Pending::Imports(uri.into(), doc.version))
            }
            _ => None,
        };
        if let (Some(id), Some(pending)) = (&meta.id, pending) {
            if self.pending.len() >= 256 {
                bail!("too many outstanding Lean validation requests");
            }
            self.pending.insert(id.clone(), pending);
        }
        Ok(())
    }
    /// Returns a replacement only for the shallow initialize response. Other
    /// traffic, including deeply nested Infoview expressions, stays opaque.
    pub fn server(&mut self, text: &str) -> Result<Option<String>> {
        let meta: Message = serde_json::from_str(text)?;
        if meta.method.as_deref() == Some("textDocument/publishDiagnostics") {
            #[derive(Deserialize)]
            struct Notification {
                params: Diagnostics,
            }
            #[derive(Deserialize)]
            struct Diagnostics {
                uri: String,
                version: Option<u64>,
                diagnostics: Vec<Diagnostic>,
                #[serde(default, rename = "isIncremental")]
                incremental: bool,
            }
            // Ignore extension data instead of recursively decoding arbitrary RPC trees.
            #[derive(Deserialize, Serialize)]
            struct Diagnostic {
                range: Value,
                severity: Option<u8>,
                message: String,
            }
            let p = serde_json::from_str::<Notification>(text)?.params;
            if let Some(doc) = self
                .documents
                .get_mut(&p.uri)
                .filter(|d| Some(d.version) == p.version)
            {
                if !p.incremental {
                    doc.diagnostics.clear();
                }
                for d in p.diagnostics {
                    doc.diagnostics.push(serde_json::to_value(d)?);
                }
            }
        } else if meta.method.is_none() {
            if let Some(pending) = meta.id.and_then(|id| self.pending.remove(&id)) {
                let value: Value = serde_json::from_str(text)?;
                if value.get("error").is_some() {
                    return Ok(None);
                }
                match pending {
                    Pending::Initialize => {
                        let mut value = value;
                        // Lean accepts full changes too. This avoids a second text
                        // editing engine on the server and lets us bind exact sources.
                        value["result"]["capabilities"]["textDocumentSync"]["change"] = json!(1);
                        return Ok(Some(value.to_string()));
                    }
                    Pending::Diagnostics(uri, version) => {
                        if let Some(doc) = self
                            .documents
                            .get_mut(&uri)
                            .filter(|d| d.version == version)
                        {
                            doc.complete = Some(version);
                        }
                    }
                    Pending::Imports(uri, version) => {
                        if let Some(doc) = self
                            .documents
                            .get_mut(&uri)
                            .filter(|d| d.version == version && d.complete == Some(version))
                        {
                            doc.imports = Some(
                                value["result"]
                                    .as_array()
                                    .context("invalid Lean imports")?
                                    .clone(),
                            );
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Block, LeanProject, Node};

    #[test]
    fn evidence_requires_native_completion_for_the_exact_saved_document() {
        let mut node = Node::new("Observed proof".into()).unwrap();
        node.blocks.push(Block {
            srctype: "lean".into(),
            content: "-- α😀\ntheorem a : True := by trivial".into(),
            ..Default::default()
        });
        let input = Input {
            project: LeanProject::default(),
            chain: vec![(node.clone(), "key".into())],
        };
        let uri = "file:///project/Lib/Test.lean";
        let mut observer = Observer::default();
        observer.prepare(uri.into(), input.clone());
        let client = |o: &mut Observer, v: Value| {
            let t = v.to_string();
            o.client(&t, &serde_json::from_str(&t).unwrap()).unwrap();
        };
        client(&mut observer, json!({"id":0,"method":"initialize"}));
        let init = observer
            .server(r#"{"id":0,"result":{"capabilities":{"textDocumentSync":{"change":2}}}}"#)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&init).unwrap()["result"]["capabilities"]
                ["textDocumentSync"]["change"],
            1
        );
        client(
            &mut observer,
            json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":node.source("lean")}}}),
        );
        assert!(observer.evidence(uri, &input, 1).is_err());
        client(
            &mut observer,
            json!({"id":1,"method":"textDocument/waitForDiagnostics","params":{"uri":uri,"version":1}}),
        );
        // A browser-supplied response is not compiler evidence.
        client(&mut observer, json!({"id":1,"result":{}}));
        assert!(observer.evidence(uri, &input, 1).is_err());
        observer.server(r#"{"id":1,"result":{}}"#).unwrap();
        client(
            &mut observer,
            json!({"id":2,"method":"$/lean/moduleHierarchy/imports","params":{"module":{"uri":uri,"name":node.module}}}),
        );
        observer.server(r#"{"id":2,"result":[]}"#).unwrap();
        assert!(observer.evidence(uri, &input, 1).is_ok());
        client(
            &mut observer,
            json!({"id":3,"method":"textDocument/waitForDiagnostics","params":{"uri":uri,"version":1}}),
        );
        client(
            &mut observer,
            json!({"method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"-- changed"}]}}),
        );
        observer.server(r#"{"id":3,"result":{}}"#).unwrap();
        assert!(observer.document(uri, &input).is_err());
        let mut changed = input.clone();
        changed.chain[0].0.blocks[0].content = "-- changed".into();
        assert!(observer.document(uri, &changed).is_ok());
        assert!(observer.evidence(uri, &changed, 2).is_err());
        changed
            .project
            .lakefile
            .push_str("\n# changed environment\n");
        assert!(observer.document(uri, &changed).is_err());
        let deep = format!(
            r#"{{"id":99,"result":{}0{}}}"#,
            "[".repeat(2048),
            "]".repeat(2048)
        );
        assert!(observer.server(&deep).unwrap().is_none());
    }
}
