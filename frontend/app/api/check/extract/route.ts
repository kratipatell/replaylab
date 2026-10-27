import {
  type DailyHit,
  buildExtractSystemPrompt,
  checkRequestSchema,
  forwardedIp,
  injectionNoRule,
  listNumericPaths,
  modelOutputSchema,
  takeDailyHit,
} from "@/lib/check";
import { LlmError, generateJson, providerLogLine } from "@/lib/llm";

const DEFAULT_DAILY_LIMIT = 20;

function dailyLimit(): number {
  const parsed = Number(process.env.CHECK_DAILY_LIMIT);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : DEFAULT_DAILY_LIMIT;
}

const hits = new Map<string, DailyHit>();

function error(message: string, status: number): Response {
  return Response.json({ status: "error", message }, { status });
}

function stripFences(raw: string): string {
  const t = raw.trim();
  if (t.startsWith("```")) {
    const withoutOpen = t.replace(/^```[a-zA-Z]*\n?/, "");
    return withoutOpen.replace(/\n?```\s*$/, "").trim();
  }
  return t;
}

export async function POST(req: Request): Promise<Response> {
  const ip = forwardedIp(req.headers.get("x-forwarded-for"));
  const decision = takeDailyHit(Date.now(), hits, ip, dailyLimit());
  if (!decision.allowed) {
    return error("Daily limit reached; try again tomorrow.", 429);
  }

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return error("Send JSON with a text field.", 400);
  }
  const parsed = checkRequestSchema.safeParse(body);
  if (!parsed.success) {
    return error(
      "Paste up to 8000 characters of transcript or caption.",
      400,
    );
  }
  const text = parsed.data.text;
  const mode = parsed.data.mode;

  const injection = injectionNoRule(text);
  if (injection) {
    return Response.json({ status: "no_rule_found", reason: injection.reason });
  }

  let raw: string;
  try {
    raw = (await generateJson(buildExtractSystemPrompt(mode), text)).trim();
    if (raw === "") {
      return error("The reader returned nothing. Try again.", 502);
    }
  } catch (e) {
    if (e instanceof LlmError) {
      console.error(providerLogLine(e));
      return error(e.message, e.httpStatus);
    }
    console.error(
      "check/extract provider failure status=unknown message=[unexpected]",
    );
    return error("The reader failed. Try again in a moment.", 502);
  }

  let json: unknown;
  try {
    json = JSON.parse(stripFences(raw));
  } catch {
    return error("The reader returned something unreadable. Try again.", 502);
  }

  const out = modelOutputSchema.safeParse(json);
  if (!out.success) {
    // TEMP DEBUG: remove after diagnosing the schema failure.
    console.error(
      "check/extract TEMP DEBUG schema issues:",
      JSON.stringify(
        out.error.issues.map((i) => ({
          path: i.path.join("."),
          code: i.code,
          message: i.message,
        })),
      ).slice(0, 1000),
    );
    return error(
      "The reader returned something unexpected. Try again with more detail in the text.",
      502,
    );
  }
  if (out.data.status === "no_rule_found") {
    return Response.json(out.data);
  }

  // Values must match the strategy at their path, so invented numbers fail.
  const slots = new Map(
    listNumericPaths(out.data.strategy).map((s) => [s.path, s.value]),
  );
  for (const p of out.data.parameters) {
    if (!slots.has(p.path)) {
      // TEMP DEBUG: remove after diagnosing the schema failure.
      console.error(
        `check/extract TEMP DEBUG unknown parameter path: ${p.path}`,
      );
      return error(
        "The reader returned something unexpected. Try again with more detail in the text.",
        502,
      );
    }
    if (p.source !== "missing") {
      const at = slots.get(p.path);
      if (p.value === null || at === undefined || p.value !== at) {
        // TEMP DEBUG: remove after diagnosing the schema failure.
        console.error(
          `check/extract TEMP DEBUG value mismatch at path: ${p.path}`,
        );
        return error(
          "The reader returned something unexpected. Try again with more detail in the text.",
          502,
        );
      }
    }
  }
  for (const path of slots.keys()) {
    if (!out.data.parameters.some((p) => p.path === path)) {
      // TEMP DEBUG: remove after diagnosing the schema failure.
      console.error(
        `check/extract TEMP DEBUG strategy value missing from parameters: ${path}`,
      );
      return error(
        "The reader returned something unexpected. Try again with more detail in the text.",
        502,
      );
    }
  }

  const missing = out.data.parameters.filter((p) => p.source === "missing");
  if (missing.length > 0) {
    return Response.json({
      status: "needs_input",
      strategy: out.data.strategy,
      parameters: out.data.parameters,
      claim: out.data.claim,
      context: out.data.context,
      missing: missing.map((p) => ({ name: p.name, path: p.path })),
      reason: `The video leaves out ${missing.map((p) => p.name).join(", ")}. Fill ${missing.length === 1 ? "it" : "them"} in to run the test.`,
    });
  }
  return Response.json({
    status: "found",
    strategy: out.data.strategy,
    parameters: out.data.parameters,
    claim: out.data.claim,
    context: out.data.context,
  });
}
