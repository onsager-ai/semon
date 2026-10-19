import type { Meta, StoryObj } from '@storybook/preact-vite';
import { NativeRouteOwner, type NativeRouteArgs } from './LegacyNativeRoute';

const meta = {
  title: 'Semon/Legacy routes',
  component: NativeRouteOwner,
  parameters: { layout: 'fullscreen', routeComposition: true },
  args: { scope: 'main' as NativeRouteArgs['scope'] },
  argTypes: { scope: { control: 'select', options: ['main', 'two-machines', 'pushed'] } },
} satisfies Meta<typeof NativeRouteOwner>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Home: Story = { args: { frame: 'home' } };
export const Drawer: Story = { args: { frame: 'drawer' } };
export const Sessions: Story = { args: { frame: 'sessions' } };
export const Machines: Story = { args: { frame: 'machines' } };
export const MachineLaptop: Story = { args: { frame: 'machine-laptop' } };
export const MachineDesktop: Story = { args: { frame: 'machine-desktop', scope: 'two-machines' } };
export const Analytics: Story = { args: { frame: 'analytics' } };
export const SessionADocs: Story = { args: { frame: 'session-a-docs' } };
export const SessionAIndex: Story = { args: { frame: 'session-a-index' } };
export const SessionAParse: Story = { args: { frame: 'session-a-parse' } };
export const SessionAReview: Story = { args: { frame: 'session-a-review' } };
export const SessionAReviewGrep: Story = { args: { frame: 'session-a-review-grep' } };
export const SessionAScan: Story = { args: { frame: 'session-a-scan' } };
export const SessionAStore: Story = { args: { frame: 'session-a-store' } };
export const SessionAdvisor: Story = { args: { frame: 'session-advisor' } };
export const SessionAtlas: Story = { args: { frame: 'session-atlas' } };
export const SessionAtlasIngest: Story = { args: { frame: 'session-atlas-ingest' } };
export const SessionDeps: Story = { args: { frame: 'session-deps' } };
export const SessionHCodex: Story = { args: { frame: 'session-h-codex' } };
export const SessionHFailed: Story = { args: { frame: 'session-h-failed' } };
export const SessionHReview: Story = { args: { frame: 'session-h-review' } };
export const SessionHReviewCodex: Story = { args: { frame: 'session-h-review-codex' } };
export const SessionHarbor: Story = { args: { frame: 'session-harbor' } };
export const SessionLSub: Story = { args: { frame: 'session-l-sub' } };
export const SessionLedger: Story = { args: { frame: 'session-ledger' } };
export const SessionPrincipal: Story = { args: { frame: 'session-principal' } };
export const SessionQCodex: Story = { args: { frame: 'session-q-codex' } };
export const SessionQPlan: Story = { args: { frame: 'session-q-plan' } };
export const SessionQuill: Story = { args: { frame: 'session-quill' } };
export const SessionSentinel: Story = { args: { frame: 'session-sentinel' } };
export const Trace_a_review_a_h_review: Story = { args: { frame: 'trace-a-review-a-h-review' } };
export const TraceAtlasH12: Story = { args: { frame: 'trace-atlas-h12' } };
export const Trace_atlas_ingest_a_ask: Story = { args: { frame: 'trace-atlas-ingest-a-ask' } };
export const TraceHReviewH3: Story = { args: { frame: 'trace-h-review-h3' } };
export const TraceHarborH1: Story = { args: { frame: 'trace-harbor-h1' } };
export const TraceLedgerH16: Story = { args: { frame: 'trace-ledger-h16' } };
export const TracePrincipalH8: Story = { args: { frame: 'trace-principal-h8' } };
export const TraceQuillH4: Story = { args: { frame: 'trace-quill-h4' } };
export const TraceSentinelSentinel0: Story = { args: { frame: 'trace-sentinel-sentinel_0' } };
