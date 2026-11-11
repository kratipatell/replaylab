import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import AgentForm from "./AgentForm";

vi.mock("./RuleBuilder", () => ({ default: () => null }));

describe("AgentForm server validation feedback", () => {
  it("shows the engine's 400 message unchanged", () => {
    const message = "max_position_pct must be > 0 and <= 100, got 101";
    const html = renderToStaticMarkup(
      <AgentForm creating={false} error={message} onCreate={() => undefined} />,
    );
    const visibleText = html
      .replaceAll("&gt;", ">")
      .replaceAll("&lt;", "<")
      .replaceAll("&amp;", "&");
    expect(visibleText).toContain(message);
  });
});
