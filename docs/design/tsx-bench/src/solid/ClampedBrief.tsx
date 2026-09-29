// A brief clamped to a few lines with "Show more", measured after layout and on every resize.
import { createSignal, onCleanup, onMount } from "solid-js";

export function ClampedBrief(p: { text: string }) {
  let body!: HTMLParagraphElement;
  const [cut, setCut] = createSignal(false), [open, setOpen] = createSignal(false);
  const measure = () => {
    if (!open()) setCut(body.scrollHeight > body.clientHeight + 1);
  };
  onMount(() => {
    const ro = new ResizeObserver(measure);
    ro.observe(body);
    measure();
    onCleanup(() => ro.disconnect());
  });
  return (
    <div class="brief-box">
      <p ref={body} class={"brief" + (open() ? " open" : cut() ? " clipped" : "")}>
        {p.text}
      </p>
      <button class="more" type="button" hidden={!cut() && !open()} aria-expanded={open()} onClick={() => setOpen(!open())}>
        {open() ? "Show less" : "Show more"}
      </button>
    </div>
  );
}
