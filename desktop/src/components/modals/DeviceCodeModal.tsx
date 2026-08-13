import { t } from "../../i18n";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Modal } from "../Modal";
import { useAppStore } from "../../store";

interface DeviceCodeModalProps {
  open: boolean;
  onClose: () => void;
  onSuccess: () => Promise<void>;
}

export function DeviceCodeModal({ open, onClose, onSuccess }: DeviceCodeModalProps) {
  const { notify } = useAppStore();
  const [pending, setPending] = useState(false);
  const [mode, setMode] = useState<"offline" | "microsoft">("microsoft");
  const [username, setUsername] = useState("");

  useEffect(() => {
    if (open) {
      setPending(false);
      setMode("microsoft");
      setUsername("");
    }
  }, [open]);

  const handleMicrosoftLogin = async () => {
    setPending(true);
    try {
      await invoke("microsoft_browser_login_cmd");
      await onSuccess();
      onClose();
    } catch (err) {
      notify(t("Microsoft sign-in failed"), String(err));
    } finally {
      setPending(false);
    }
  };

  const handleAddOffline = async () => {
    setPending(true);
    try {
      await invoke("add_offline_account_cmd", { username });
      await onSuccess();
      onClose();
    } catch (err) {
      notify(t("Could not add offline account"), String(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <Modal open={open} onClose={onClose} title={t("Add account")}>
      <div className="account-login-content">
        <div className="account-mode-tabs">
          <button
            className={`account-mode-tab ${mode === "microsoft" ? "active" : ""}`}
            onClick={() => setMode("microsoft")}
            disabled={pending}
          >{t("Microsoft")}
          </button>
          <button
            className={`account-mode-tab ${mode === "offline" ? "active" : ""}`}
            onClick={() => setMode("offline")}
            disabled={pending}
          >{t("Offline")}
          </button>
        </div>

        {mode === "microsoft" ? (
          <>
            <div className="account-login-hero">
              <div className="account-login-mark">
                <svg width="28" height="28" viewBox="0 0 24 24" aria-hidden="true">
                  <path fill="#f35325" d="M1 1h10v10H1z" />
                  <path fill="#81bc06" d="M13 1h10v10H13z" />
                  <path fill="#05a6f0" d="M1 13h10v10H1z" />
                  <path fill="#ffba08" d="M13 13h10v10H13z" />
                </svg>
              </div>
              <strong>{t("Sign in to Minecraft")}</strong>
              <span>{t("A secure Microsoft window will open. VelGrinor never sees your password.")}</span>
            </div>
            <button className="btn btn-primary account-login-button" onClick={handleMicrosoftLogin} disabled={pending}>
              {pending ? t("Waiting for Microsoft…") : t("Continue with Microsoft")}
            </button>
            {pending && <p className="account-login-pending">{t("Complete sign-in in the Microsoft window.")}</p>}
          </>
        ) : (
          <>
            <p className="account-login-description">{t("Play without Microsoft sign-in. Online-mode servers are unavailable with this account.")}
            </p>
            <input
              className="input"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && username.trim().length >= 3) void handleAddOffline();
              }}
              placeholder={t("Minecraft username")}
              maxLength={16}
              autoFocus
            />
            <button
              className="btn btn-primary"
              onClick={handleAddOffline}
              disabled={pending || username.trim().length < 3}
            >
              {pending ? t("Adding…") : t("Add offline account")}
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}
