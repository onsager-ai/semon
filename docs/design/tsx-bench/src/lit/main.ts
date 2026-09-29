// The sample's entry: the tooltip, the session tree fed by a simulated live update, and a clamped brief.
import { installTooltip } from "../shared/tooltip";
import { makeModel, step } from "../shared/fixture";
import { mountSessionTree } from "./SessionTree";
import { mountClampedBrief } from "./ClampedBrief";

installTooltip();
let model = makeModel(), tick = 0, current: string | undefined;
const lanes = document.querySelector<HTMLElement>("#lanes"), brief = document.querySelector<HTMLElement>("#brief");
if (lanes) {
  const tree = mountSessionTree(lanes, (id) => {
    current = id;
    tree.update(model, current);
  });
  tree.update(model, current);
  setInterval(() => {
    model = step(model, ++tick);
    tree.update(model, current);
  }, 2000);
}
if (brief) mountClampedBrief(brief, model.sessions[0]?.brief ?? "");
