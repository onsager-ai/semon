// The sample's entry: the tooltip, the session tree fed by a simulated live update, and a clamped brief.
import { h } from "./jsx";
import { installTooltip } from "../shared/tooltip";
import { makeModel, step } from "../shared/fixture";
import { mountSessionTree } from "./SessionTree";
import { ClampedBrief } from "./ClampedBrief";

installTooltip();
let model = makeModel(), tick = 0, current: string | undefined;
const tree = mountSessionTree((id) => {
  current = id;
  tree.update(model, current);
});
document.querySelector("#lanes")?.append(tree.el);
document.querySelector("#brief")?.append(<ClampedBrief text={model.sessions[0]?.brief ?? ""} />);
tree.update(model, current);
setInterval(() => {
  model = step(model, ++tick);
  tree.update(model, current);
}, 2000);
