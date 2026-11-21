import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  createAgent,
  getTemplate,
  getAgentDecisions,
  getAllAgentEquity,
  listTemplates,
  listAgents,
  postWalkForward,
  type WalkForwardRequest,
  type StrategyTemplate,
} from "./api";
import { sampleWalkForwardReport } from "./walkforward.fixture";
import capturedTemplates from "./__fixtures__/templates/response.json";

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function stubFetch(implementation: typeof fetch): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(implementation);
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("agent API client", () => {
  it("unwraps and parses the agent list response", async () => {
    stubFetch(async () =>
      jsonResponse({
        agents: [{
          id: "ag-1",
          name: "BTC paper",
          status: "running",
          symbol: "BTCUSDT",
          interval: "1h",
          run_id: null,
          created_at: 1,
          started_at: 2,
          stopped_at: null,
          equity: 10_200,
          last_price: 102,
          last_ts: 3,
          last_error: null,
        }],
      }),
    );

    const agents = await listAgents();
    expect(agents).toHaveLength(1);
    expect(agents[0]?.status).toBe("running");
    expect(agents[0]?.equity).toBe(10_200);
  });

  it("posts typed creation data and parses the created detail", async () => {
    const fetchMock = stubFetch(async () =>
      jsonResponse({ agent: { id: "ag-new" }, strategy: {}, risk: {}, halt: null }, 201),
    );
    const body = {
      name: "Test",
      strategy: {
        asset: "BTCUSDT",
        timeframe: "1h",
        side: "long" as const,
        entry: { all: [], any: [] },
      },
      risk: {
        max_position_pct: 100,
        max_daily_loss_pct: 10,
        max_drawdown_pct: 20,
      },
    };

    const result = await createAgent(body);
    expect(result.agent.id).toBe("ag-new");
    const [, init] = fetchMock.mock.calls[0] ?? [];
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual(body);
  });

  it("preserves the engine's 400 validation message for agent creation", async () => {
    const message = "max_position_pct must be > 0 and <= 100, got 101";
    stubFetch(async () => jsonResponse({ error: message }, 400));
    await expect(createAgent({
      name: "Test",
      strategy: {
        asset: "BTCUSDT",
        timeframe: "1h",
        side: "long",
        entry: { all: [], any: [] },
      },
    })).rejects.toMatchObject({ status: 400, message });
  });

  it("parses decisions and requests older pages with the cursor", async () => {
    const fetchMock = stubFetch(async () =>
      jsonResponse({ decisions: [], next_before: null }),
    );
    await getAgentDecisions("ag 1", { limit: 100, before: "123:2" });
    const [url] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toContain("/agents/ag%201/decisions?");
    expect(String(url)).toContain("before=123%3A2");
  });

  it("follows equity cursors until the retained history is loaded", async () => {
    let call = 0;
    stubFetch(async () => {
      call += 1;
      return call === 1
        ? jsonResponse({ equity: [{ bar_ts: 100, equity: 10_000 }], next_since: 101 })
        : jsonResponse({ equity: [{ bar_ts: 200, equity: 10_100 }], next_since: null });
    });

    await expect(getAllAgentEquity("ag-1")).resolves.toEqual([
      { bar_ts: 100, equity: 10_000 },
      { bar_ts: 200, equity: 10_100 },
    ]);
    expect(call).toBe(2);
  });

  it("preserves API status codes for UI recovery messages", async () => {
    stubFetch(async () => jsonResponse({ error: "kill switch is on" }, 409));
    await expect(listAgents()).rejects.toMatchObject({
      status: 409,
      message: "kill switch is on",
    });
  });
});

describe("walk-forward API client", () => {
  const body: WalkForwardRequest = {
    strategy: {
      asset: "BTCUSDT",
      timeframe: "1h",
      side: "long",
      entry: { all: [], any: [] },
    },
    start_ms: 1,
    end_ms: 2,
    windows: 4,
    is_bars: 1000,
    oos_bars: 500,
    mode: "rolling",
    params: [{ target: "entry.all.0.right", kind: "float", values: [99, 100] }],
    search: { method: "grid" },
    objective: "sharpe",
  };

  it("posts the request to /walk-forward and parses the report", async () => {
    const report = sampleWalkForwardReport();
    const fetchMock = stubFetch(async () => jsonResponse(report));

    await expect(postWalkForward(body)).resolves.toEqual(report);
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toMatch(/\/walk-forward$/);
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual(body);
  });

  it("preserves the engine's 400 message for rejected settings", async () => {
    const message = "not enough candles (10) for 4 windows of 1000 IS and 500 OOS bars (need 3000)";
    stubFetch(async () => jsonResponse({ error: message }, 400));
    await expect(postWalkForward(body)).rejects.toMatchObject({ status: 400, message });
  });

  it("keeps the 429 status so the UI can explain the rate limit", async () => {
    stubFetch(async () => jsonResponse({ error: "too many optimizer requests; try again shortly" }, 429));
    const error = await postWalkForward(body).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 429 });
  });

  it("rejects with an AbortError when the caller cancels", async () => {
    stubFetch(async (_url, init) => {
      if (init?.signal?.aborted) {
        throw Object.assign(new Error("aborted"), { name: "AbortError" });
      }
      return jsonResponse(sampleWalkForwardReport());
    });
    const controller = new AbortController();
    controller.abort();
    await expect(postWalkForward(body, controller.signal)).rejects.toMatchObject({ name: "AbortError" });
  });
});

describe("strategy template API client", () => {
  const templates = (capturedTemplates as unknown as { templates: StrategyTemplate[] }).templates;

  it("lists templates from the engine response wrapper", async () => {
    const fetchMock = stubFetch(async () => jsonResponse({ templates }));
    await expect(listTemplates()).resolves.toEqual(templates);
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toMatch(/\/templates$/);
    expect(init?.method).toBeUndefined();
  });

  it("loads one template by its encoded ID", async () => {
    const template = templates[0];
    if (template === undefined) {
      throw new Error("captured template response is empty");
    }
    const fetchMock = stubFetch(async () => jsonResponse(template));
    await expect(getTemplate(template.id)).resolves.toEqual(template);
    const [url] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toMatch(new RegExp(`/templates/${template.id}$`));
  });
});
