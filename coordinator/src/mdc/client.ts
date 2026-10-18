// The HTTP side of `mdc`: one request with the caller's token, errors with stable reasons.

/**
 * A failure with a stable `reason` code; `uncertain` when a sent request may have taken effect;
 * `messages` is LeanGround's explanation, e.g. Lean's errors for a rejected proof.
 */
/** A rejected request body's problems, one per field, e.g. `text: String must contain at most 500 character(s)`. */
function issues(value: any): string | undefined {
  if (!Array.isArray(value?.issues) || !value.issues.length) return undefined;
  return value.issues
    .map((i: { path?: unknown[]; message?: string }) => `${(i.path ?? []).join(".") || "body"}: ${i.message}`)
    .join("; ");
}

export class MdcError extends Error {
  constructor(
    public reason: string,
    public details?: string,
    public status?: number,
    public uncertain = false,
    public messages?: string,
  ) {
    super(
      status
        ? `HTTP ${status}: ${details ?? reason}`
        : details
          ? `${reason}: ${details}`
          : reason,
    );
  }
  toJSON() {
    return {
      reason: this.reason,
      ...(this.status ? { status: this.status } : {}),
      ...(this.details ? { details: this.details } : {}),
      // The request may have taken effect: rerun the same command to replay it.
      ...(this.uncertain ? { uncertain: true } : {}),
      ...(this.messages ? { messages: this.messages } : {}),
    };
  }
}

export class Client {
  constructor(
    public origin: string,
    private token: string,
    private fetcher: typeof fetch = fetch,
  ) {}
  async request(
    method: string,
    path: string,
    body?: unknown,
    headers: Record<string, string> = {},
  ): Promise<any> {
    // Once a request may have left, its effect is unknown until a response is read.
    const lost = () =>
      new MdcError(
        "unreachable",
        `cannot reach MathDoc at ${this.origin} (MDC_URL)`,
        undefined,
        method !== "GET",
      );
    let response: Response;
    let text: string;
    try {
      response = await this.fetcher(`${this.origin}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${this.token}`,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
          ...headers,
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      text = await response.text();
    } catch {
      throw lost();
    }
    let value: any = null;
    try {
      value = text ? JSON.parse(text) : null;
    } catch {
      value = { error: text };
    }
    if (!response.ok)
      throw new MdcError(
        value?.reason ?? "request_failed",
        value?.error ?? issues(value) ?? value?.reason ?? "request failed",
        response.status,
        // A server error rolls its transaction back, but a proxy's may hide a commit.
        method !== "GET" && response.status >= 500,
      );
    return value;
  }
  board(project: string): Promise<any> {
    return this.request("GET", `/api/coordination/projects/${project}`);
  }
  /**
   * Send a command checked against the current state (claim, heartbeat, finish, submit, note…;
   * docs/mathdoc.md §2.7.5) with `If-Match: *`, without reading the board first.
   */
  current(
    project: string,
    body: unknown,
    key: string,
  ): Promise<{ revision: number; result: any; job?: string }> {
    return this.request(
      "POST",
      `/api/coordination/projects/${project}/commands`,
      body,
      { "if-match": "*", "idempotency-key": key },
    );
  }
}
