import { z } from "zod";
import { Fault } from "./domain.js";
export class RemoteError extends Fault {
  constructor(
    code: string,
    public retryable: boolean,
    public remoteStatus: number,
  ) {
    super(code, 502);
  }
}
export async function responseJson(res: Response): Promise<unknown> {
  const text = await res.text();
  if (text.length > 4000000)
    throw new RemoteError("upstream_response_too_large", false, res.status);
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    throw new RemoteError("invalid_upstream_json", false, res.status);
  }
  if (!res.ok) {
    const code = z
      .object({ reason: z.string().optional() })
      .passthrough()
      .safeParse(data);
    throw new RemoteError(
      code.success
        ? (code.data.reason ?? `upstream_${res.status}`)
        : `upstream_${res.status}`,
      res.status >= 500 || res.status === 429,
      res.status,
    );
  }
  return data;
}
export class LeanGround {
  constructor(
    private url: string,
    private token: string,
  ) {
    const u = new URL(url);
    if (!["http:", "https:"].includes(u.protocol) || u.username || u.password)
      throw new Error("invalid LeanGround URL");
  }
  async call(path: string, body?: unknown): Promise<any> {
    // External payloads are validated at their domain boundary.
    let res: Response;
    try {
      res = await fetch(`${this.url.replace(/\/$/, "")}${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          authorization: `Bearer ${this.token}`,
          "content-type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(120000),
        redirect: "error",
      });
    } catch {
      throw new RemoteError("leanground_unavailable", true, 0);
    }
    return responseJson(res);
  }
  fact(operation: string, body: unknown) {
    return this.call(`/v1/facts/${operation}`, body);
  }
}
