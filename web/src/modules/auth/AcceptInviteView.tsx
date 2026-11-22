import { type FormEvent, useEffect, useState } from "react";
import { AlertCircle, Eye, EyeOff, Lock, Mail, User } from "lucide-react";
import {
  type AuthSessionView,
  type InvitationPreview,
  acceptInvitation,
  previewInvitation,
} from "../../lib/api";

const MIN_PASSWORD = 12;

/** Reads `#invite=<64 hex>` from the URL fragment (kept out of server logs and Referer). */
export function readInviteTokenFromHash(hash: string): string | null {
  const match = /^#invite=([0-9a-f]{64})$/i.exec(hash);
  return match ? match[1] : null;
}

interface AcceptInviteViewProps {
  token: string;
  onAuthenticated: (session: AuthSessionView) => void;
  onCancel: () => void;
}

export function AcceptInviteView({ token, onAuthenticated, onCancel }: AcceptInviteViewProps) {
  const [preview, setPreview] = useState<InvitationPreview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    previewInvitation(token)
      .then((p) => {
        if (!cancelled) setPreview(p);
      })
      .catch(() => {
        if (!cancelled) {
          setLoadError("This invitation link is invalid, expired, or has already been used.");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [token]);

  const isNewAccount = preview !== null && !preview.account_exists;

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!preview) return;
    if (isNewAccount) {
      if (!name.trim()) {
        setError("Please enter your name.");
        return;
      }
      if (password.length < MIN_PASSWORD || password.length > 256) {
        setError(`Password must be between ${MIN_PASSWORD} and 256 characters.`);
        return;
      }
      if (password !== confirm) {
        setError("Passwords do not match.");
        return;
      }
    } else if (!password) {
      setError("Please enter your existing password.");
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      const session = await acceptInvitation({
        token,
        name: isNewAccount ? name.trim() : undefined,
        password,
      });
      onAuthenticated(session);
    } catch (err) {
      setError(
        err instanceof Error && err.message === "invalid credentials"
          ? "Incorrect password for this account."
          : err instanceof Error && err.message === "too many requests"
            ? "Too many attempts. Please wait a minute and try again."
            : "Could not accept the invitation. Check your details and try again.",
      );
    } finally {
      setSubmitting(false);
    }
  }

  const scopeText = preview
    ? preview.scope === "workspace"
      ? `workspace ${preview.workspace_name ?? ""}`.trim()
      : "all workspaces"
    : "";

  return (
    <div className="min-h-screen flex flex-col bg-[var(--color-bg-muted)] text-[var(--color-text)]">
      <header className="h-14 px-4 sm:px-6 bg-[var(--color-bg)] border-b border-[var(--color-border)] flex items-center gap-3">
        <span className="w-7 h-7 rounded-[6px] bg-[var(--color-primary)] text-white font-bold text-sm inline-flex items-center justify-center shrink-0">
          M
        </span>
        <span className="font-bold text-base tracking-tight">MaxiLoop</span>
      </header>

      <main className="flex-1 flex items-center justify-center p-4 sm:p-6">
        <section className="w-full max-w-[440px] bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg p-6 sm:p-8 shadow-[var(--pm-shadow-card)]">
          <h1 className="text-[22px] font-bold leading-tight">Accept invitation</h1>

          {loadError ? (
            <>
              <div
                role="alert"
                className="mt-4 p-3 rounded-md bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-start gap-2"
              >
                <AlertCircle size={15} className="shrink-0 mt-0.5" />
                <span>{loadError} Ask your admin for a new link.</span>
              </div>
              <button
                type="button"
                onClick={onCancel}
                className="mt-5 w-full h-10 rounded-[6px] border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] text-sm font-semibold cursor-pointer"
              >
                Go to sign in
              </button>
            </>
          ) : !preview ? (
            <p className="mt-3 text-sm text-[var(--color-text-dim)]">Checking invitation...</p>
          ) : (
            <>
              <p className="mt-1 text-sm text-[var(--color-text-muted)]">
                You were invited to <strong>{preview.tenant_name}</strong> as{" "}
                <span className="pm-lozenge">{preview.role}</span> ({scopeText}).
              </p>
              <p className="mt-2 text-xs text-[var(--color-text-dim)]">
                {isNewAccount
                  ? "Choose your name and a password to create your account."
                  : "You already have a MaxiLoop account. Enter its password to join."}
              </p>

              {error && (
                <div
                  role="alert"
                  className="mt-4 p-3 rounded-md bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-start gap-2"
                >
                  <AlertCircle size={15} className="shrink-0 mt-0.5" />
                  <span>{error}</span>
                </div>
              )}

              <form onSubmit={handleSubmit} className="mt-5 space-y-4">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5">
                    Email
                  </label>
                  <div className="relative">
                    <Mail
                      size={15}
                      className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                    />
                    <input
                      type="email"
                      readOnly
                      value={preview.email}
                      autoComplete="username"
                      className="pm-input pl-10 h-10 text-sm"
                    />
                  </div>
                </div>

                {isNewAccount && (
                  <div>
                    <label
                      htmlFor="invite-name"
                      className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5"
                    >
                      Your name
                    </label>
                    <div className="relative">
                      <User
                        size={15}
                        className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                      />
                      <input
                        id="invite-name"
                        type="text"
                        required
                        maxLength={128}
                        autoComplete="name"
                        value={name}
                        onChange={(e) => setName(e.target.value)}
                        className="pm-input pl-10 h-10 text-sm"
                      />
                    </div>
                  </div>
                )}

                <div>
                  <label
                    htmlFor="invite-password"
                    className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5"
                  >
                    {isNewAccount ? `Password (min ${MIN_PASSWORD} characters)` : "Password"}
                  </label>
                  <div className="relative">
                    <Lock
                      size={15}
                      className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                    />
                    <input
                      id="invite-password"
                      type={showPassword ? "text" : "password"}
                      required
                      autoComplete={isNewAccount ? "new-password" : "current-password"}
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      className="pm-input pl-10 pr-10 h-10 text-sm"
                    />
                    <button
                      type="button"
                      onClick={() => setShowPassword((prev) => !prev)}
                      aria-label={showPassword ? "Hide password" : "Show password"}
                      aria-pressed={showPassword}
                      className="absolute right-2.5 top-1/2 -translate-y-1/2 p-1 rounded text-[var(--color-text-dim)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
                    >
                      {showPassword ? <EyeOff size={15} /> : <Eye size={15} />}
                    </button>
                  </div>
                </div>

                {isNewAccount && (
                  <div>
                    <label
                      htmlFor="invite-confirm"
                      className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5"
                    >
                      Confirm password
                    </label>
                    <input
                      id="invite-confirm"
                      type={showPassword ? "text" : "password"}
                      required
                      autoComplete="new-password"
                      value={confirm}
                      onChange={(e) => setConfirm(e.target.value)}
                      className="pm-input h-10 text-sm"
                    />
                  </div>
                )}

                <button
                  type="submit"
                  disabled={submitting}
                  className="w-full h-10 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-60 text-white font-semibold text-sm cursor-pointer"
                >
                  {submitting ? "Joining..." : isNewAccount ? "Create account & join" : "Join"}
                </button>
                <button
                  type="button"
                  onClick={onCancel}
                  className="w-full text-xs font-medium text-[var(--color-text-dim)] hover:text-[var(--color-text)] cursor-pointer"
                >
                  Cancel
                </button>
              </form>
            </>
          )}
        </section>
      </main>
    </div>
  );
}
