/** Measured host geometry. Fixed property names and finite numbers are the entire input language. */
export type GeometrySlot =
  | 'paddingBottom'
  | 'scrollPaddingTop'
  | 'barHeight'
  | 'accountLeft'
  | 'accountWidth'
  | 'accountBottom'
  | 'intrinsicHeight';
const properties: Record<GeometrySlot, string> = {
  paddingBottom: 'padding-bottom',
  scrollPaddingTop: 'scroll-padding-top',
  barHeight: '--barh',
  accountLeft: '--account-left',
  accountWidth: '--account-width',
  accountBottom: '--account-bottom',
  intrinsicHeight: 'contain-intrinsic-block-size',
};
let sheet: CSSStyleSheet | null = null,
  observer: MutationObserver | null = null,
  serial = 0;
const records = new Map<HTMLElement, { id: number; values: Map<GeometrySlot, number> }>();
// Measured values replace former inline declarations and must outrank responsive defaults.
function paint() {
  if (!sheet) {
    sheet = new CSSStyleSheet();
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  }
  sheet.replaceSync('');
  for (const [node, record] of records) {
    if (!node.isConnected) {
      records.delete(node);
      continue;
    }
    const declarations = [...record.values]
      .map(
        ([slot, value]) =>
          properties[slot] +
          ':' +
          (slot === 'intrinsicHeight' ? 'auto ' : '') +
          value +
          'px !important',
      )
      .join(';');
    if (declarations)
      sheet.insertRule(
        '[data-semon-geometry="' + record.id + '"]{' + declarations + '}',
        sheet.cssRules.length,
      );
  }
}
export function setGeometry(node: HTMLElement, slot: GeometrySlot, value: number | null) {
  if (
    !Object.hasOwn(properties, slot) ||
    (value !== null && (!Number.isFinite(value) || Math.abs(value) > 1e8))
  )
    throw new Error('Invalid host geometry');
  let record = records.get(node);
  if (!record) {
    record = { id: ++serial, values: new Map() };
    records.set(node, record);
    node.dataset.semonGeometry = String(record.id);
  }
  if (value === null) record.values.delete(slot);
  else record.values.set(slot, value);
  paint();
  if (!observer) {
    observer = new MutationObserver(() => {
      if ([...records.keys()].some((node) => !node.isConnected)) paint();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
  }
}
/** Release measurements when the host owning these document nodes is torn down. */
export function releaseGeometry(root: HTMLElement, slots?: readonly GeometrySlot[]) {
  for (const [node, record] of records) {
    if (slots ? node === root : node === root || root.contains(node) || !node.isConnected) {
      if (slots) for (const slot of slots) record.values.delete(slot);
      if (!slots || !record.values.size) {
        records.delete(node);
        delete node.dataset.semonGeometry;
      }
    }
  }
  if (records.size) paint();
  else {
    observer?.disconnect();
    observer = null;
    if (sheet)
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((value) => value !== sheet);
    sheet = null;
  }
}
export function revealMeasuredTurn(node: HTMLElement, visible: boolean) {
  node.classList.toggle('semon-measuring-turn', visible);
}
