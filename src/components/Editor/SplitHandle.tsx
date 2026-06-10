import { createSignal } from "solid-js";

export function SplitHandle(props: {
  onResize: (deltaX: number) => void;
  onDoubleClick: () => void;
}) {
  const [active, setActive] = createSignal(false);

  function onPointerDown(e: PointerEvent) {
    e.preventDefault();
    setActive(true);
    let lastX = e.clientX;
    const target = e.currentTarget as HTMLElement;
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
      class={`split-handle${active() ? " active" : ""}`}
      onPointerDown={onPointerDown}
      onDblClick={props.onDoubleClick}
    />
  );
}
