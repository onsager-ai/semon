import type { CatalogReadScope } from './sessionRoute';

export type ViewerRoute =
  | { v: 'home' | 'analytics' | 'sessions' | 'machines' }
  | { v: 'host'; id: string }
  | { v: 'machine'; id: string }
  | { v: 'session'; id: string; turn?: string; readScope?: CatalogReadScope }
  | { v: 'trace'; sid: string; turn: string; readScope: CatalogReadScope };
