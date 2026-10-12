// One small seam for reading video text with a swappable AI provider.
// Server code only: keys stay out of the browser. Never log keys or user text.

import { GoogleGenAI } from "@google/genai";

export interface JsonReader {
  generateJson(system: string, user: string): Promise<string>;
}

export type LlmFailure = "missing_key" | "rate_limited" | "unavailable";

export class LlmError extends Error {
  kind: LlmFailure;
  status: number | null;
  detail: string | null;
  httpStatus: number;

  constructor(
    kind: LlmFailure,
    message: string,
    opts?: { status?: number | null; detail?: string | null; http?: number },
  ) {
    super(message);
    this.name = "LlmError";
    this.kind = kind;
    this.status = opts?.status ?? null;
    this.detail = opts?.detail ?? null;
    this.httpStatus =
      opts?.http ?? (kind === "missing_key" ? 500 : kind === "rate_limited" ? 503 : 502);
  }
}

function errorStatus(e: unknown): number | null {
  if (typeof e !== "object" || e === null) {
    return null;
  }
  const record = e as Record<string, unknown>;
  for (const key of ["status", "code"]) {
    const v = record[key];
    if (typeof v === "number" && Number.isInteger(v)) {
      return v;
    }
    if (typeof v === "string" && /^\d{3}$/.test(v.trim())) {
      return Number(v.trim());
    }
  }
  const response = record["response"];
  if (typeof response === "object" && response !== null) {
    const v = (response as Record<string, unknown>)["status"];
    if (typeof v === "number" && Number.isInteger(v)) {
      return v;
    }
  }
  return null;
}

function errorMessage(e: unknown): string {
  if (typeof e !== "object" || e === null) {
    return "";
  }
  const message = (e as Record<string, unknown>)["message"];
  return typeof message === "string" ? message : "";
}

function mentionsQuota(message: string): boolean {
  return (
    /RESOURCE_EXHAUSTED/i.test(message) ||
    /quota/i.test(message) ||
    /rate.?limit/i.test(message) ||
    message.includes("429")
  );
}

// Maps an SDK failure to a safe, user-facing message. The raw provider text
// rides along in `detail` for the server log only, never for the page.
export function toLlmError(e: unknown): LlmError {
  if (e instanceof LlmError) {
    return e;
  }
  const status = errorStatus(e);
  const message = errorMessage(e);
  const detail = message === "" ? null : message;
  if (status === 429 || mentionsQuota(message)) {
    return new LlmError(
      "rate_limited",
      "The free AI service is busy or out of quota; try again later.",
      { status, detail },
    );
  }
  if (status === 401 || status === 403) {
    return new LlmError("unavailable", "The AI key was rejected. Check GEMINI_API_KEY.", {
      status,
      detail,
    });
  }
  if (status === 404 || (/model/i.test(message) && /not.?found/i.test(message))) {
    return new LlmError("unavailable", "The AI model was not found. Check GEMINI_MODEL.", {
      status,
      detail,
    });
  }
  const mentionsModel = /model/i.test(message);
  const mentionsInvalidArg =
    /INVALID_ARGUMENT/i.test(message) || /invalid argument/i.test(message);
  if (
    mentionsInvalidArg ||
    (status === 400 && (mentionsModel || /invalid/i.test(message)))
  ) {
    return new LlmError(
      "unavailable",
      "The AI model name is not valid. Check GEMINI_MODEL.",
      { status, detail },
    );
  }
  return new LlmError(
    "unavailable",
    `The AI service failed (code ${status === null ? "unknown" : status}).`,
    { status, detail },
  );
}

// Key-shaped text must never reach the logs.
export function sanitizeLogMessage(text: string): string {
  const out = text
    .replace(/AIza[0-9A-Za-z_\-]{10,}/g, "[redacted]")
    .replace(/sk-[A-Za-z0-9_\-]{8,}/g, "[redacted]")
    .replace(
      /(api[_-]?key\s*[:=]\s*['"]?)[A-Za-z0-9_\-.~+/=]{6,}['"]?/gi,
      "$1[redacted]",
    );
  return out.slice(0, 300);
}

export function providerLogLine(e: LlmError): string {
  const status = e.status === null ? "unknown" : String(e.status);
  return `check/extract provider failure status=${status} message=${sanitizeLogMessage(e.detail ?? e.message)}`;
}

class GeminiReader implements JsonReader {
  constructor(
    private apiKey: string,
    private model: string,
  ) {}

  async generateJson(system: string, user: string): Promise<string> {
    try {
      const ai = new GoogleGenAI({ apiKey: this.apiKey });
      const res = await ai.models.generateContent({
        model: this.model,
        contents: user,
        config: {
          systemInstruction: system,
          responseMimeType: "application/json",
          temperature: 0,
          maxOutputTokens: 2000,
        },
      });
      const text = res.text?.trim() ?? "";
      if (text === "") {
        throw new LlmError("unavailable", "The reader returned nothing.");
      }
      return text;
    } catch (e) {
      throw toLlmError(e);
    }
  }
}

export function llmReader(): JsonReader {
  const name = (process.env.LLM_PROVIDER ?? "gemini").trim().toLowerCase();
  if (name === "gemini") {
    const apiKey = process.env.GEMINI_API_KEY;
    const model = process.env.GEMINI_MODEL;
    if (!apiKey || !model) {
      throw new LlmError("missing_key", "AI is not configured.");
    }
    return new GeminiReader(apiKey, model);
  }
  throw new LlmError(
    "unavailable",
    `Unknown AI provider "${name}". Set LLM_PROVIDER to "gemini".`,
    { http: 500 },
  );
}

export function generateJson(system: string, user: string): Promise<string> {
  return llmReader().generateJson(system, user);
}
