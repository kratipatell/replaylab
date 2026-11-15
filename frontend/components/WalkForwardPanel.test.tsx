import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ApiError } from "@/lib/api";
import { defaultForm } from "@/lib/testlab";
import { classifyWalkForwardError } from "@/lib/walkforward";
import WalkForwardPanel, { RunProblemNotice } from "./WalkForwardPanel";

vi.mock("./RuleBuilder", () => ({ default: () => null }));

function visible(html: string): string {
  return html
    .replaceAll("&gt;", ">")
    .replaceAll("&lt;", "<")
    .replaceAll("&amp;", "&")
    .replaceAll("&#x27;", "'")
    .replaceAll("&quot;", '"');
}

describe("walk-forward error states", () => {
  it("shows the engine's 400 message unchanged", () => {
    const message = "not enough candles (10) for 4 windows of 1000 IS and 500 OOS bars (need 3000)";
    const problem = classifyWalkForwardError(new ApiError(400, message));
    const html = renderToStaticMarkup(<RunProblemNotice problem={problem} />);
    expect(html).toContain('role="alert"');
    expect(visible(html)).toContain(message);
    expect(visible(html)).toContain("The engine rejected these settings:");
  });

  it("explains a 429 as a rate limit to retry later", () => {
    const problem = classifyWalkForwardError(new ApiError(429, "too many optimizer requests; try again shortly"));
    const html = renderToStaticMarkup(<RunProblemNotice problem={problem} />);
    expect(visible(html)).toContain("Too many runs right now:");
    expect(visible(html)).toContain("Wait a minute, then try again.");
    expect(visible(html)).not.toContain("too many optimizer requests");
  });

  it("gives a timeout a hint about making the run smaller", () => {
    const problem = classifyWalkForwardError(new ApiError(408, "walk-forward timed out after 60s"));
    const html = renderToStaticMarkup(<RunProblemNotice problem={problem} />);
    expect(visible(html)).toContain("walk-forward timed out after 60s");
    expect(visible(html)).toContain("Try fewer windows");
  });

  it("reports a cancelled run as a status, not an error", () => {
    const problem = classifyWalkForwardError(Object.assign(new Error("aborted"), { name: "AbortError" }));
    const html = renderToStaticMarkup(<RunProblemNotice problem={problem} />);
    expect(html).toContain('role="status"');
    expect(html).not.toContain('role="alert"');
    expect(visible(html)).toContain("Run cancelled.");
  });
});

describe("WalkForwardPanel before the first run", () => {
  it("shows the empty state and the optimizer note", () => {
    const html = visible(
      renderToStaticMarkup(<WalkForwardPanel form={defaultForm()} onFormChange={() => undefined} />),
    );
    expect(html).toContain("No walk-forward run yet.");
    expect(html).toContain("Run walk-forward");
    expect(html).toContain("runs the optimizer once per window, so it can take a while");
    expect(html).not.toContain("Cancel run");
  });
});
