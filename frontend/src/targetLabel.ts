import type { TargetDto } from "./api";

type TargetSummary = Pick<TargetDto, "name" | "kind" | "address">;

const targetKindLabels: Record<TargetDto["kind"], string> = {
  lxc: "LXC-Container",
  linux_server: "Linux-Server",
  windows_server: "Windows-System",
};

export function targetDisplayLabel(target: TargetSummary) {
  return `${target.name} · ${targetKindLabels[target.kind]} · ${target.address}`;
}
