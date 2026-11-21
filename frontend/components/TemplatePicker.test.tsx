import { isValidElement, type ReactElement, type ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { StrategyTemplate } from "@/lib/api";
import capturedTemplates from "@/lib/__fixtures__/templates/response.json";
import TemplatePicker from "./TemplatePicker";

const templates = (capturedTemplates as unknown as { templates: StrategyTemplate[] }).templates;

type ButtonProps = {
  children?: ReactNode;
  onClick?: () => void;
};

function firstButton(node: ReactNode): ReactElement<ButtonProps> | null {
  if (Array.isArray(node)) {
    for (const child of node) {
      const found = firstButton(child);
      if (found !== null) {
        return found;
      }
    }
    return null;
  }
  if (!isValidElement(node)) {
    return null;
  }
  const element = node as ReactElement<ButtonProps>;
  if (element.type === "button" && element.props.onClick !== undefined) {
    return element;
  }
  return firstButton(element.props.children);
}

describe("TemplatePicker", () => {
  it("renders cards from the captured real engine response", () => {
    const html = renderToStaticMarkup(
      <TemplatePicker
        templates={templates}
        loading={false}
        error={null}
        selectedId={null}
        onSelect={() => undefined}
        onRetry={() => undefined}
      />,
    );

    expect(templates).toHaveLength(6);
    for (const template of templates) {
      expect(html).toContain(template.name);
      expect(html).toContain(template.market_idea);
      expect(html).toContain(template.typical_failure_mode);
    }
    expect(html).toContain("not promises of future performance");
  });

  it("selects a template from a card", () => {
    const onSelect = vi.fn<(template: StrategyTemplate) => void>();
    const tree = TemplatePicker({
      templates,
      loading: false,
      error: null,
      selectedId: null,
      onSelect,
      onRetry: () => undefined,
    });
    const button = firstButton(tree);

    expect(button).not.toBeNull();
    button?.props.onClick?.();
    expect(onSelect).toHaveBeenCalledWith(templates[0]);
  });

  it("shows loading, empty, and recoverable error states", () => {
    const loading = renderToStaticMarkup(
      <TemplatePicker
        templates={[]}
        loading
        error={null}
        selectedId={null}
        onSelect={() => undefined}
        onRetry={() => undefined}
      />,
    );
    expect(loading).toContain("Loading strategy templates");

    const empty = renderToStaticMarkup(
      <TemplatePicker
        templates={[]}
        loading={false}
        error={null}
        selectedId={null}
        onSelect={() => undefined}
        onRetry={() => undefined}
      />,
    );
    expect(empty).toContain("No strategy templates are available");

    const onRetry = vi.fn();
    const error = renderToStaticMarkup(
      <TemplatePicker
        templates={[]}
        loading={false}
        error="engine offline"
        selectedId={null}
        onSelect={() => undefined}
        onRetry={onRetry}
      />,
    );
    expect(error).toContain('role="alert"');
    expect(error).toContain("engine offline");
    expect(error).toContain("Retry");
  });
});
