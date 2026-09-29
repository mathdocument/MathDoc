import { readFileSync } from "node:fs";
import { z } from "zod";

/**
 * A secret from NAME or from the file named by NAME_FILE (compose secrets). Setting both is
 * an error, as is an empty file; values are trimmed of surrounding whitespace.
 */
export function secret(
  name: string,
  env: NodeJS.ProcessEnv = process.env,
): string | undefined {
  const direct = env[name];
  const file = env[`${name}_FILE`];
  if (direct && file)
    throw new Error(`set only one of ${name} and ${name}_FILE`);
  if (!file) return direct || undefined;
  let value: string;
  try {
    value = readFileSync(file, "utf8").trim();
  } catch {
    throw new Error(`cannot read ${name}_FILE ${file}`);
  }
  if (!value) throw new Error(`${name}_FILE ${file} is empty`);
  return value;
}

/** MDC_DATABASE_URL, with the password from MDC_DATABASE_PASSWORD(_FILE) when given. */
export function databaseUrl(env: NodeJS.ProcessEnv = process.env): string {
  const url = env.MDC_DATABASE_URL;
  if (!url) throw new Error("MDC_DATABASE_URL required");
  const password = secret("MDC_DATABASE_PASSWORD", env);
  if (!password) return url;
  const parsed = new URL(url);
  parsed.password = encodeURIComponent(password);
  return parsed.toString();
}
const actorSchema = z
  .object({ token: z.string().min(16), admin: z.boolean().default(false) })
  .strict();
export interface Config {
  dsn: string;
  host: string;
  port: number;
  actors: Record<string, z.infer<typeof actorSchema>>;
  leanUrl: string;
  leanToken: string;
  leanActor: string;
  terminusUrl?: string;
  terminusUser: string;
  terminusPassword?: string;
  /** Coordination prototype (app/); when set it is served at / instead of web/. */
  appDir?: string;
  /** The retained knowledge editor (web/), served at / and under /p/<db>/<branch>/. */
  webDir?: string;
  /** public_origin of the legacy backend: an HTTP(S) origin accepted besides loopback. */
  publicOrigin?: string;
}
export function config(): Config {
  const actors = z
    .record(actorSchema)
    .parse(JSON.parse(secret("MDC_ACTORS") ?? "{}"));
  if (
    !Object.keys(actors).length ||
    new Set(Object.values(actors).map((x) => x.token)).size !==
      Object.keys(actors).length
  )
    throw new Error("MDC_ACTORS requires distinct actor tokens");
  for (const name of Object.keys(actors))
    if (!/^[A-Za-z0-9_-]+$/.test(name)) throw new Error("invalid actor name");
  const need = (k: string) => {
    const v = process.env[k];
    if (!v) throw new Error(`${k} required`);
    return v;
  };
  return {
    dsn: databaseUrl(),
    host: process.env.MDC_HOST ?? "127.0.0.1",
    port: z.coerce
      .number()
      .int()
      .min(1)
      .max(65535)
      .parse(process.env.MDC_PORT ?? 17843),
    actors,
    leanUrl: need("LEANGROUND_SERVER_URL"),
    leanToken: secret("LEANGROUND_FACT_TOKEN") ?? need("LEANGROUND_FACT_TOKEN"),
    leanActor: need("LEANGROUND_ACTOR"),
    terminusUrl: process.env.MDC_TERMINUS_URL,
    terminusUser: process.env.MDC_TERMINUS_USER ?? "admin",
    terminusPassword: secret("MDC_TERMINUS_PASSWORD"),
    appDir: process.env.MDC_APP_DIR,
    webDir: process.env.MDC_WEB_DIR ?? "web/dist",
    publicOrigin: publicOrigin(process.env.MDC_PUBLIC_ORIGIN),
  };
}

function publicOrigin(value?: string): string | undefined {
  if (!value) return undefined;
  const url = new URL(value);
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  )
    throw new Error(
      "MDC_PUBLIC_ORIGIN must be an HTTP(S) origin without credentials, path, query or fragment",
    );
  return url.origin;
}
