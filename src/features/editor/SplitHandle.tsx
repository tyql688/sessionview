import { type KeyboardEvent, type PointerEvent as ReactPointerEvent, useState } from "react";

interface SplitHandleProps {
  label: string;
  valueNow: number;
  onResize: (deltaX: number) => void;
  onDoubleClick: () => void;
}

export function SplitHandle(props: SplitHandleProps) {
  const [active, setActive] = useState(false);

  function onPointerDown(event: ReactPointerEvent<HTMLHRElement>) {
    event.preventDefault();
    setActive(true);
    let lastX = event.clientX;
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);

    function onPointerMove(pointerEvent: PointerEvent) {
      props.onResize(pointerEvent.clientX - lastX);
      lastX = pointerEvent.clientX;
    }

    function onPointerUp() {
      setActive(false);
      target.releasePointerCapture(event.pointerId);
      target.removeEventListener("pointermove", onPointerMove);
      target.removeEventListener("pointerup", onPointerUp);
    }

    target.addEventListener("pointermove", onPointerMove);
    target.addEventListener("pointerup", onPointerUp);
  }

  function onKeyDown(event: KeyboardEvent<HTMLHRElement>) {
    const step = event.shiftKey ? 32 : 8;
    if (event.key === "ArrowLeft") props.onResize(-step);
    else if (event.key === "ArrowRight") props.onResize(step);
    else if (event.key === "Enter") props.onDoubleClick();
    else return;
    event.preventDefault();
  }

  return (
    <hr
      className={`split-handle${active ? " active" : ""}`}
      aria-label={props.label}
      aria-orientation="vertical"
      aria-valuemin={15}
      aria-valuemax={85}
      aria-valuenow={Math.round(props.valueNow)}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onDoubleClick={props.onDoubleClick}
    />
  );
}
