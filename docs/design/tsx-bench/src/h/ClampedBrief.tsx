// A brief clamped to a few lines with "Show more": the button shows only while the text is cut off, measured after
// layout and again whenever the box resizes (fonts landing, the column narrowing).
import { h } from "./jsx";

export function ClampedBrief({ text }: { text: string }) {
  let body!: HTMLElement, more!: HTMLButtonElement;
  const measure = () => {
    if (!body.isConnected || body.classList.contains("open")) return;
    const cut = body.scrollHeight > body.clientHeight + 1;
    more.hidden = !cut;
    body.classList.toggle("clipped", cut);
  };
  const el = (
    <div class="brief-box">
      <p class="brief" ref={(n) => (body = n as HTMLElement)}>
        {text}
      </p>
      <button
        class="more"
        type="button"
        hidden
        aria-expanded="false"
        ref={(n) => (more = n as HTMLButtonElement)}
        onClick={() => {
          const opened = body.classList.toggle("open");
          more.textContent = opened ? "Show less" : "Show more";
          more.setAttribute("aria-expanded", String(opened));
        }}
      >
        Show more
      </button>
    </div>
  );
  new ResizeObserver(measure).observe(body);
  requestAnimationFrame(measure);
  return el;
}
