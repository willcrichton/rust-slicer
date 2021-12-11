import { CallFlowistry } from "./vsc_utils";
import { display_subcmd_results } from "./utils";

export async function slice(
  call_flowistry: CallFlowistry,
  direction: "backward" | "forward",
  type: "highlight" | "select",
  flags: string = ""
) {
  await display_subcmd_results(
    call_flowistry,
    "Slice",
    `${direction}_slice`,
    type,
    flags
  );
}
