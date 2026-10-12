import { afterEach, describe, expect, it } from "vitest";
import {
  LlmError,
  generateJson,
  llmReader,
  providerLogLine,
  sanitizeLogMessage,
  toLlmError,
} from "./llm";

const SAVED = {
  LLM_PROVIDER: process.env.LLM_PROVIDER,
  GEMINI_API_KEY: process.env.GEMINI_API_KEY,
  GEMINI_MODEL: process.env.GEMINI_MODEL,
};

afterEach(() => {
  process.env.LLM_PROVIDER = SAVED.LLM_PROVIDER;
  process.env.GEMINI_API_KEY = SAVED.GEMINI_API_KEY;
  process.env.GEMINI_MODEL = SAVED.GEMINI_MODEL;
});

function clearEnv(): void {
  delete process.env.LLM_PROVIDER;
  delete process.env.GEMINI_API_KEY;
  delete process.env.GEMINI_MODEL;
}

describe("llm provider", () => {
  it("defaults to gemini and fails without a key", () => {
    clearEnv();
    try {
      llmReader();
      expect.unreachable("gemini without a key must fail");
    } catch (e) {
      expect(e).toBeInstanceOf(LlmError);
      expect((e as LlmError).kind).toBe("missing_key");
      expect((e as LlmError).message).toBe("AI is not configured.");
    }
  });

  it("fails without a model and leaks no key details", () => {
    clearEnv();
    process.env.GEMINI_API_KEY = "secret-key-material";
    try {
      llmReader();
      expect.unreachable("gemini without a model must fail");
    } catch (e) {
      expect(e).toBeInstanceOf(LlmError);
      expect((e as LlmError).message).not.toContain("secret-key-material");
    }
  });

  it("rejects an unknown provider with a clear error", () => {
    clearEnv();
    process.env.LLM_PROVIDER = "mystery";
    try {
      llmReader();
      expect.unreachable("unknown provider must fail");
    } catch (e) {
      expect(e).toBeInstanceOf(LlmError);
      expect((e as LlmError).message).toMatch(/unknown.*mystery/i);
      expect((e as LlmError).message).toMatch(/gemini/);
    }
  });

  it("generateJson surfaces the provider error without calling a model", async () => {
    clearEnv();
    try {
      await generateJson("sys", "user");
      expect.unreachable("generateJson without a key must fail");
    } catch (e) {
      expect(e).toBeInstanceOf(LlmError);
      expect((e as LlmError).kind).toBe("missing_key");
    }
  });

  it("treats an empty model id as not configured", () => {
    clearEnv();
    process.env.GEMINI_API_KEY = "key";
    process.env.GEMINI_MODEL = "";
    try {
      llmReader();
      expect.unreachable("empty model must fail");
    } catch (e) {
      expect(e).toBeInstanceOf(LlmError);
      expect((e as LlmError).kind).toBe("missing_key");
      expect((e as LlmError).message).toBe("AI is not configured.");
    }
  });

  it("accepts any configured model id with no hardcoded fallback", () => {
    clearEnv();
    process.env.GEMINI_API_KEY = "key";
    process.env.GEMINI_MODEL = "some-future-free-model";
    expect(() => llmReader()).not.toThrow();
  });
});

describe("provider error mapping", () => {
  it("maps a bad model name on 400", () => {
    const e = toLlmError({
      status: 400,
      message: "INVALID_ARGUMENT: model 'bogus' is not valid",
    });
    expect(e.message).toBe(
      "The AI model name is not valid. Check GEMINI_MODEL.",
    );
  });

  it("maps a missing model on 404", () => {
    const e = toLlmError({ status: 404, message: "models/x is not found" });
    expect(e.message).toBe(
      "The AI model was not found. Check GEMINI_MODEL.",
    );
  });

  it("maps a rejected key on 401 and 403", () => {
    for (const status of [401, 403]) {
      const e = toLlmError({ status, message: "API key not valid" });
      expect(e.message).toBe(
        "The AI key was rejected. Check GEMINI_API_KEY.",
      );
    }
  });

  it("maps 429 and quota wording to busy", () => {
    const limited = toLlmError({ status: 429, message: "Too many requests" });
    expect(limited.kind).toBe("rate_limited");
    expect(limited.message).toBe(
      "The free AI service is busy or out of quota; try again later.",
    );
    const quota = toLlmError({ message: "free quota exceeded for the day" });
    expect(quota.kind).toBe("rate_limited");
    expect(quota.message).toBe(
      "The free AI service is busy or out of quota; try again later.",
    );
  });

  it("keeps the status code for anything else", () => {
    expect(toLlmError({ status: 500, message: "boom" }).message).toBe(
      "The AI service failed (code 500).",
    );
    expect(toLlmError({}).message).toBe(
      "The AI service failed (code unknown).",
    );
  });
});

describe("provider log line", () => {
  it("logs status and message with keys stripped", () => {
    const line = providerLogLine(
      toLlmError({ status: 401, message: "key AIzaSyD-abc123XYZ789 rejected" }),
    );
    expect(line).toContain("status=401");
    expect(line).not.toContain("AIza");
    expect(line).toContain("[redacted]");
  });

  it("strips api key patterns", () => {
    const line = providerLogLine(
      toLlmError({ status: 500, message: "bad api_key=SECRETVALUE123 here" }),
    );
    expect(line).not.toContain("SECRETVALUE123");
    expect(line).toContain("[redacted]");
  });

  it("truncates long messages to 300 characters", () => {
    expect(sanitizeLogMessage("x".repeat(400)).length).toBe(300);
  });
});
