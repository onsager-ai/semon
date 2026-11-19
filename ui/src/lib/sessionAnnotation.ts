export interface SessionAnnotation {
  system: string;
  repository: string;
  ticket: string;
  issue: number | null;
  pr: number | null;
  title: string | null;
  source: 'annotation';
}

export function annotationProducer(annotation: SessionAnnotation): string {
  return annotation.system === 'ostrom' ? 'Ostrom' : annotation.system;
}

export function annotationLink(
  annotation: SessionAnnotation,
  kind: 'issues' | 'pull',
  number: number,
): string {
  return (
    'https://github.com/' +
    annotation.repository.split('/').map(encodeURIComponent).join('/') +
    '/' +
    kind +
    '/' +
    number
  );
}
