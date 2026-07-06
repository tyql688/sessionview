import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TitleBar } from "./TitleBar";

function requireElement<T extends Element>(element: T | null, name: string): T {
  if (!element) throw new Error(`Missing ${name}`);
  return element;
}

function renderTitleBar(
  overrides: Partial<Parameters<typeof TitleBar>[0]> = {},
) {
  const props = {
    showWindowControls: true,
    isMaximized: false,
    onMinimize: vi.fn(),
    onToggleMaximize: vi.fn(),
    onClose: vi.fn(),
    onStartDragging: vi.fn(),
    ...overrides,
  };

  const view = render(<TitleBar {...props} />);
  return { ...view, props };
}

describe("TitleBar", () => {
  it("starts window dragging on primary single mouse down", () => {
    const { container, props } = renderTitleBar();
    const titlebar = requireElement(
      container.querySelector(".titlebar"),
      "titlebar",
    );

    fireEvent.mouseDown(titlebar, { buttons: 1, detail: 1 });

    expect(props.onStartDragging).toHaveBeenCalledTimes(1);
    expect(props.onToggleMaximize).not.toHaveBeenCalled();
  });

  it("toggles maximize on primary double mouse down", () => {
    const { container, props } = renderTitleBar();
    const titlebar = requireElement(
      container.querySelector(".titlebar"),
      "titlebar",
    );

    fireEvent.mouseDown(titlebar, { buttons: 1, detail: 2 });

    expect(props.onToggleMaximize).toHaveBeenCalledTimes(1);
    expect(props.onStartDragging).not.toHaveBeenCalled();
  });

  it("ignores titlebar dragging from window control buttons", () => {
    const { container, props } = renderTitleBar();
    const minimizeButton = requireElement(
      container.querySelector<HTMLButtonElement>(".win-ctrl-btn"),
      "minimize button",
    );

    fireEvent.mouseDown(minimizeButton, { buttons: 1, detail: 1 });
    fireEvent.click(minimizeButton);

    expect(props.onStartDragging).not.toHaveBeenCalled();
    expect(props.onMinimize).toHaveBeenCalledTimes(1);
  });

  it("hides window controls when disabled", () => {
    const { container } = renderTitleBar({ showWindowControls: false });

    expect(container.querySelector(".win-controls")).toBeNull();
  });
});
