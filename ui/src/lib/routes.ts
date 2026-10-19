export type ViewerRoute =
  | { v: 'home' | 'analytics' | 'sessions' | 'machines' }
  | { v: 'host'; id: string }
  | { v: 'machine'; id: string }
  | { v: 'session'; id: string; turn?: string }
  | { v: 'trace'; sid: string; turn: string };
