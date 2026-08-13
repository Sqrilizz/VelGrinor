import { t } from "../../i18n";
import { useEffect, useRef, useState } from "react";
import { Modal } from "../Modal";
import type { Profile } from "../../types";

interface ProfileJsonModalProps {
  open: boolean;
  profile: Profile | null;
  onClose: () => void;
}

const highlightJson = (value: string) => {
  const escaped = value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");

  return escaped.replace(
    /("(\\u[a-fA-F0-9]{4}|\\[^u]|[^\\"])*"(?:\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g,
    (match) => {
      let cls = "json-token-number";
      if (match[0] === "\"") {
        cls = match.endsWith(":") ? "json-token-key" : "json-token-string";
      } else if (match === "true" || match === "false") {
        cls = "json-token-boolean";
      } else if (match === "null") {
        cls = "json-token-null";
      }
      return `<span class="${cls}">${match}</span>`;
    }
  );
};

export function ProfileJsonModal({ open, profile, onClose }: ProfileJsonModalProps) {
  const json = profile ? JSON.stringify(profile, null, 2) : "No profile";
  const [copied, setCopied] = useState(false);
  const copyTimeoutRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current !== null) {
        window.clearTimeout(copyTimeoutRef.current);
      }
    };
  }, []);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(json);
      setCopied(true);
      if (copyTimeoutRef.current !== null) {
        window.clearTimeout(copyTimeoutRef.current);
      }
      copyTimeoutRef.current = window.setTimeout(() => {
        setCopied(false);
      }, 1400);
    } catch {
      setCopied(false);
    }
  };

  return (
    <Modal open={open} onClose={onClose} title={t("Profile JSON")} large>
      <pre className="json-viewer">
        <button
          className={`btn-icon json-copy-btn${copied ? " copied" : ""}`}
          onClick={handleCopy}
          title={copied ? t("Copied") : t("Copy JSON")}
          aria-label={t("Copy JSON")}
          type="button"
        >
          {copied ? (
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6">
              <path d="M3.5 8.5l3 3 6-6" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
              <rect x="6" y="5" width="7" height="9" rx="2" />
              <rect x="3" y="2" width="7" height="9" rx="2" />
            </svg>
          )}
        </button>
        <code
          className="json-code"
          dangerouslySetInnerHTML={{ __html: highlightJson(json) }}
        />
      </pre>
    </Modal>
  );
}
