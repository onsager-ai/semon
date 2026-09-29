// A brief clamped to a few lines with "Show more", measured after layout and on every resize.
import { useLayoutEffect, useRef, useState } from "preact/hooks";

export function ClampedBrief({ text }: { text: string }) {
  const body = useRef<HTMLParagraphElement>(null);
  const [cut, setCut] = useState(false), [open, setOpen] = useState(false);
  useLayoutEffect(() => {
    const n = body.current;
    if (!n) return;
    const measure = () => {
      if (!open) setCut(n.scrollHeight > n.clientHeight + 1);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(n);
    measure();
    return () => ro.disconnect();
  }, [text, open]);
  return (
    <div class="brief-box">
      <p ref={body} class={"brief" + (open ? " open" : cut ? " clipped" : "")}>
        {text}
      </p>
      <button class="more" type="button" hidden={!cut && !open} aria-expanded={open} onClick={() => setOpen(!open)}>
        {open ? "Show less" : "Show more"}
      </button>
    </div>
  );
}
