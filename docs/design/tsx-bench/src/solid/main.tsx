// The sample's entry: the tooltip, the session tree fed by a simulated live update, and a clamped brief.
import { createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { render } from "solid-js/web";
import { installTooltip } from "../shared/tooltip";
import { makeModel, step } from "../shared/fixture";
import type { Session } from "../shared/model";
import { SessionTree } from "./SessionTree";
import { ClampedBrief } from "./ClampedBrief";

installTooltip();
let tick = 0;
const [model, setModel] = createSignal(makeModel()), [current, setCurrent] = createSignal<string | undefined>();
// Sessions by id in a store: reconcile() writes only the fields an update changed, so only their bindings run.
const [byId, setById] = createStore<Record<string, Session>>({});
const adopt = () => setById(reconcile(Object.fromEntries(model().sessions.map((s) => [s.id, s]))));
adopt();
const lanes = document.querySelector("#lanes"), brief = document.querySelector("#brief");
if (lanes) render(() => <SessionTree model={model} byId={(id) => byId[id]} current={current} onOpen={(id) => setCurrent(id)} />, lanes);
if (brief) render(() => <ClampedBrief text={model().sessions[0]?.brief ?? ""} />, brief);
setInterval(() => {
  setModel(step(model(), ++tick));
  adopt();
}, 2000);
