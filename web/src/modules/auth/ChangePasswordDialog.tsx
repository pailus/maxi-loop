import { type FormEvent, useEffect, useState } from "react";
import { AlertCircle, X } from "lucide-react";
import { ApiError, changePassword } from "../../lib/api";

const MIN_PASSWORD = 12;

interface ChangePasswordDialogProps {
  email: string;
  onClose: () => void;
  onChanged: () => void;
}

export function ChangePasswordDialog({ email, onClose, onChanged }: ChangePasswordDialogProps) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (next.length < MIN_PASSWORD || next.length > 256) {
      setError(`New password must be between ${MIN_PASSWORD} and 256 characters.`);
      return;
    }
    if (next !== confirm) {
      setError("New passwords do not match.");
      return;
    }
    if (next === current) {
      setError("New password must be different from the current one.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      await changePassword({ current_password: current, new_password: next });
      onChanged();
    } catch (err) {
      setError(
        err instanceof ApiError && err.status === 403
          ? "Current password is incorrect."
          : err instanceof ApiError && err.status === 429
            ? "Too many attempts. Please wait a minute and try again."
            : "Could not change the password. Please try again.",
      );
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[10vh] px-4">
      <div className="absolute inset-0 bg-[rgba(9,30,66,0.54)] pm-fade-in" onClick={onClose} />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="change-password-title"
        className="relative w-full max-w-[420px] rounded-xl bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-raised)] pm-fade-in"
      >
        <div className="px-5 py-3.5 border-b border-[var(--color-border)] flex items-center">
          <h2 id="change-password-title" className="text-base font-semibold flex-1">
            Change password
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-dim)] cursor-pointer"
          >
            <X size={16} />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-5 space-y-3">
          {/* Lets password managers associate the new password with the account. */}
          <input type="email" value={email} autoComplete="username" readOnly hidden />
          <p className="text-xs text-[var(--color-text-dim)]">
            Your other signed-in sessions will be signed out.
          </p>

          {error && (
            <div
              role="alert"
              className="p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-start gap-2"
            >
              <AlertCircle size={14} className="shrink-0 mt-0.5" />
              <span>{error}</span>
            </div>
          )}

          <div>
            <label htmlFor="cp-current" className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
              Current password
            </label>
            <input
              id="cp-current"
              type="password"
              required
              autoComplete="current-password"
              value={current}
              onChange={(e) => setCurrent(e.target.value)}
              className="pm-input"
            />
          </div>
          <div>
            <label htmlFor="cp-new" className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
              New password (min {MIN_PASSWORD} characters)
            </label>
            <input
              id="cp-new"
              type="password"
              required
              autoComplete="new-password"
              value={next}
              onChange={(e) => setNext(e.target.value)}
              className="pm-input"
            />
          </div>
          <div>
            <label htmlFor="cp-confirm" className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
              Confirm new password
            </label>
            <input
              id="cp-confirm"
              type="password"
              required
              autoComplete="new-password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              className="pm-input"
            />
          </div>

          <div className="pt-1 flex justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className="h-9 px-3 rounded-[6px] text-sm font-medium hover:bg-[var(--color-bg-hover)] cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={submitting}
              className="h-9 px-4 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-60 text-white text-sm font-semibold cursor-pointer"
            >
              {submitting ? "Saving..." : "Change password"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
