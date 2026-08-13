import { t } from "../i18n";
import ReactDOM from "react-dom";
import clsx from "clsx";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title?: string;
  large?: boolean;
  className?: string;
  children: React.ReactNode;
}

export function Modal({ open, onClose, title, large, className, children }: ModalProps) {
  if (!open) return null;
  return ReactDOM.createPortal(
    <div className="modal-backdrop" onClick={onClose}>
      <div className={clsx("modal", large && "modal-lg", className)} onClick={(e) => e.stopPropagation()}>
        <div className={clsx("modal-header", !title && "modal-header--titleless")}>
          {title && <h3 className="modal-title">{title}</h3>}
          <button className="btn-icon modal-close" onClick={onClose} aria-label={t("Close")} type="button">
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.5">
              <path d="M3 3l8 8M11 3l-8 8" strokeLinecap="round" />
            </svg>
          </button>
        </div>
        <div className="modal-body">
          {children}
        </div>
      </div>
    </div>,
    document.body
  );
}
