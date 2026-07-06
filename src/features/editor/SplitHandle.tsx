import { type PointerEvent as ReactPointerEvent, useState } from "react";

export function SplitHandle(props: {
  onResize: (deltaX: number) => void;
  onDoubleClick: () => void;
}) {
  const [active, setActive] = useState(false);

  function onPointerDown(e: ReactPointerEvent<HTMLDivElement>) {
    e.preventDefault();
    setActive(true);
    let lastX = e.clientX;
    const target = e.currentTarget;
    target.setPointerCapture(e.pointerId);

    function onPointerMove(ev: PointerEvent) {
      props.onResize(ev.clientX - lastX);
      lastX = ev.clientX;
    }

    function onPointerUp() {
      setActive(false);
      target.releasePointerCapture(e.pointerId);
      target.removeEventListener("pointermove", onPointerMove);
      target.removeEventListener("pointerup", onPointerUp);
    }

    target.addEventListener("pointermove", onPointerMove);
    target.addEventListener("pointerup", onPointerUp);
  }

  return (
    <div
      className={`split-handle${active ? " active" : ""}`}
      onPointerDown={onPointerDown}
      onDoubleClick={props.onDoubleClick}
    />
  );
}
