import { t } from "../../i18n";
import { Modal } from "../Modal";
import type { LaunchPlan } from "../../types";

interface LaunchPlanModalProps {
  open: boolean;
  plan: LaunchPlan | null;
  onClose: () => void;
}

export function LaunchPlanModal({ open, plan, onClose }: LaunchPlanModalProps) {
  return (
    <Modal open={open} onClose={onClose} title={t("Launch plan")} large>
      {plan && (
        <div style={{ fontSize: 13, fontFamily: "var(--font-mono)", display: "flex", flexDirection: "column", gap: 8 }}>
          <div><span style={{ color: "rgba(255,255,255,0.5)" }}>{t("instance:")}</span> {plan.instance_dir}</div>
          <div><span style={{ color: "rgba(255,255,255,0.5)" }}>{t("java:")}</span> {plan.java_exec}</div>
          <div><span style={{ color: "rgba(255,255,255,0.5)" }}>{t("main class:")}</span> {plan.main_class}</div>
          <div><span style={{ color: "rgba(255,255,255,0.5)" }}>{t("jvm args:")}</span> {plan.jvm_args.join(" ")}</div>
          <div><span style={{ color: "rgba(255,255,255,0.5)" }}>{t("game args:")}</span> {plan.game_args.join(" ")}</div>
        </div>
      )}
    </Modal>
  );
}
