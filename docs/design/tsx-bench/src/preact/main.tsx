// The sample's entry: the tooltip, the session tree fed by a simulated live update, and a clamped brief.
import { render } from "preact";
import { installTooltip } from "../shared/tooltip";
import { makeModel, step } from "../shared/fixture";
import { SessionTree } from "./SessionTree";
import { ClampedBrief } from "./ClampedBrief";

installTooltip();
let model = makeModel(), tick = 0, current: string | undefined;
const lanes = document.querySelector("#lanes"), brief = document.querySelector("#brief");
const draw = () => {
  if (lanes) render(<SessionTree model={model} current={current} onOpen={(id) => ((current = id), draw())} />, lanes);
};
if (brief) render(<ClampedBrief text={model.sessions[0]?.brief ?? ""} />, brief);
draw();
setInterval(() => {
  model = step(model, ++tick);
  draw();
}, 2000);
