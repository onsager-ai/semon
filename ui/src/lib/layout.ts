/** Numeric geometry only. No application HTML/style props or arbitrary CSS text. */
export type LayoutProperty =
  | 'width'
  | 'height'
  | 'left'
  | 'top'
  | 'margin-left'
  | 'min-width'
  | 'max-height'
  | 'max-width';
const properties: readonly string[] = [
  'width',
  'height',
  'left',
  'top',
  'margin-left',
  'min-width',
  'max-height',
  'max-width',
];
let serial = 0;
export function createMeasuredLayout() {
  const sheet = new CSSStyleSheet(),
    prefix = 'semon-geometry-' + ++serial + '-';
  const rules = new Map<string, string>();
  let disposed = false;
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  return {
    reset() {
      if (disposed) return;
      rules.clear();
      sheet.replaceSync('');
    },
    className(property: LayoutProperty, value: number, unit: 'px' | '%'): string {
      if (disposed) throw new Error('Measured layout is destroyed');
      if (
        !properties.includes(property) ||
        !Number.isFinite(value) ||
        Math.abs(value) > 1e8 ||
        !['px', '%'].includes(unit)
      )
        throw new Error('Invalid measured geometry');
      const key = property + ':' + value + unit,
        existing = rules.get(key);
      if (existing) return existing;
      const name = prefix + rules.size;
      sheet.insertRule(
        '.' + name + '{' + property + ':' + value + unit + '}',
        sheet.cssRules.length,
      );
      rules.set(key, name);
      return name;
    },
    destroy() {
      if (disposed) return;
      disposed = true;
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((value) => value !== sheet);
      rules.clear();
      sheet.replaceSync('');
    },
  };
}
