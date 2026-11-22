import { afterEach, describe, expect, it, vi } from "vitest";

const { generateJson } = vi.hoisted(() => ({
  generateJson: vi.fn(),
}));

vi.mock("@/lib/llm", () => ({
  LlmError: class LlmError extends Error {
    readonly httpStatus = 500;
  },
  generateJson,
  providerLogLine: () => "provider failure",
}));

import { POST } from "./route";

function request(ip: string): Request {
  return new Request("http://localhost/api/check/extract", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-forwarded-for": ip,
    },
    body: JSON.stringify({
      mode: "video",
      text: "Enter when close crosses above the 20-period SMA; exit on a cross below.",
    }),
  });
}

afterEach(() => {
  vi.unstubAllEnvs();
  vi.clearAllMocks();
});

describe("Check third-party inference route", () => {
  it("is disabled in demo mode unless explicitly enabled", async () => {
    vi.stubEnv("DEMO_MODE", "true");
    vi.stubEnv("CHECK_API_ENABLED", "false");

    const response = await POST(request("203.0.113.20"));

    expect(response.status).toBe(503);
    expect(await response.json()).toMatchObject({
      status: "error",
      message: expect.stringContaining("Check is disabled"),
    });
    expect(generateJson).not.toHaveBeenCalled();
  });

  it("allows explicit opt-in and enforces the per-IP daily limit", async () => {
    vi.stubEnv("DEMO_MODE", "true");
    vi.stubEnv("CHECK_API_ENABLED", "true");
    vi.stubEnv("CHECK_DAILY_LIMIT", "1");
    generateJson.mockResolvedValue(
      JSON.stringify({
        status: "no_rule_found",
        reason: "No actionable trading rule found.",
      }),
    );

    const first = await POST(request("203.0.113.21"));
    const second = await POST(request("203.0.113.21"));

    expect(first.status).toBe(200);
    expect(second.status).toBe(429);
    expect(generateJson).toHaveBeenCalledTimes(1);
  });
});
