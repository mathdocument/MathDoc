import { z } from "zod";
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
  appDir: string;
}
export function config(): Config {
  const actors = z
    .record(actorSchema)
    .parse(JSON.parse(process.env.MDC_ACTORS ?? "{}"));
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
    dsn: need("MDC_DATABASE_URL"),
    host: process.env.MDC_HOST ?? "127.0.0.1",
    port: z.coerce
      .number()
      .int()
      .min(1)
      .max(65535)
      .parse(process.env.MDC_PORT ?? 17843),
    actors,
    leanUrl: need("LEANGROUND_SERVER_URL"),
    leanToken: need("LEANGROUND_FACT_TOKEN"),
    leanActor: need("LEANGROUND_ACTOR"),
    terminusUrl: process.env.MDC_TERMINUS_URL,
    terminusUser: process.env.MDC_TERMINUS_USER ?? "admin",
    terminusPassword: process.env.MDC_TERMINUS_PASSWORD,
    appDir: process.env.MDC_APP_DIR ?? "app/dist",
  };
}
