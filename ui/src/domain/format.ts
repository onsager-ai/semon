export const clock = (t: number, NOW: number) => { const d = new Date(t), n = new Date(NOW); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };
export const ago = (t: number, NOW: number) => { const d = Math.floor((NOW - t) / 60000); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };
export const dur = (a: number, b: number | null | undefined, NOW: number) => { const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 60000)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };
export const tok = (m: number) => m >= 1 ? m.toFixed(1) + "M" : Math.round(m * 1000) + "k";

export const shortName = (name: string) => { const h = String(name).split(".")[0]; return h.length > 14 ? h.slice(0, 14) + "…" : h; };

  // Short display names for machines, `[[id, full name]]` in, a Map of id to name out. A long name is cut to 14 characters; two that
  // cut alike keep their tails ("build-…-east-1"), then their whole first label, then their id, until no two in the list are alike.
export const machineShorts = (names: Iterable<readonly [string, string]>) => {
    const forms = (id: string, full: string) => { const first = String(full).split(".")[0]; return [shortName(first), first.length > 14 ? first.slice(0, 6) + "…" + first.slice(-7) : first, first, id]; };
    const opts = new Map([...names].map(([id, full]) => [id, forms(id, full)])), level = new Map([...opts.keys()].map((id) => [id, 0]));
    for (let step = 0; step < 3; step++) {
      const groups = new Map<string, string[]>(); for (const [id, o] of opts) { const l = o[level.get(id)!]; if (!groups.has(l)) groups.set(l, []); groups.get(l)!.push(id); }
      let clash = false; for (const ids of groups.values()) if (ids.length > 1) { clash = true; for (const id of ids) level.set(id, level.get(id)! + 1); }
      if (!clash) break;
    }
    return new Map([...opts].map(([id, o]) => [id, o[level.get(id)!]]));
  };

export const shortModel = (model: string | null | undefined) => String(model ?? "Unknown model").replace(/^gpt-(\d+\.\d+)-(.+)$/i, "$2 $1").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");

export const clean = (t: string) => t.replace(/[`*]/g, "");


export const liveUrl = (u: string) => { try { return /^https?:$/.test(new URL(u).protocol) && /^https?:\/\//i.test(u) ? u : null; } catch { return null; } };
  // A one-line preview (Home): block markers dropped, lines run together.
export const preview = (t: string) => t.split("\n").map((l) => l.replace(/^\s*(#{1,6}\s+|>\s?|[-*]\s+|\d{1,3}[.)]\s+)/, "").trim()).filter((l) => l && !/^\s*\|?\s*:?-{2,}/.test(l)).join(" ");

export const compactCount = (n: number) => { if (n < 1e3) return String(n); if (n < 1e4) return +(n / 1e3).toFixed(1) + "k"; const k = Math.round(n / 1e3); return k < 1e3 ? k + "k" : +(n / 1e6).toFixed(1) + "M"; };

export const niceStep = (max: number) => [.25, .5, 1, 2, 5, 10, 20, 50, 100, 200, 500].find((x) => x * 3 >= max) ?? 1000;
export function timeText(ms: number) { const mins = Math.max(0, Math.round(ms / 60000)), days = Math.floor(mins / 1440), hours = Math.floor(mins % 1440 / 60), rem = mins % 60; return days ? days + "d " + hours + "h" : hours ? hours + "h " + rem + "m" : mins + "m"; }
export const countText = (n: number) => Math.round(n).toLocaleString(), hLabel = (n: number) => n ? +n.toFixed(2) + " h" : "0";
