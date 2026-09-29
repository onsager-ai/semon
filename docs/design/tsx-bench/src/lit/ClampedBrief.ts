// A brief clamped to a few lines with "Show more", measured after layout and on every resize. lit-html has no component
// lifecycle, so the state lives in the mount's closure and a ref callback wires the ResizeObserver.
import { html, render } from "lit-html";
import { ref } from "lit-html/directives/ref.js";

export function mountClampedBrief(box: HTMLElement, text: string): void {
  let cut = false, open = false, ro: ResizeObserver | null = null;
  const measure = (n: Element) => {
    const next = !open && n.scrollHeight > n.clientHeight + 1;
    if (next !== cut) {
      cut = next;
      draw();
    }
  };
  const watch = (n: Element | undefined) => {
    if (!n || ro) return;
    ro = new ResizeObserver(() => measure(n));
    ro.observe(n);
  };
  function draw(): void {
    render(
      html`<div class="brief-box">
        <p class=${"brief" + (open ? " open" : cut ? " clipped" : "")} ${ref(watch)}>${text}</p>
        <button class="more" type="button" ?hidden=${!cut && !open} aria-expanded=${String(open)} @click=${() => ((open = !open), draw())}>${open ? "Show less" : "Show more"}</button>
      </div>`,
      box,
    );
  }
  draw();
}
