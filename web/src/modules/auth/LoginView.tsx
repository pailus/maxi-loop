import { type FormEvent, useState } from "react";
import { AlertCircle, Eye, EyeOff, Lock, Mail } from "lucide-react";
import { type AuthSessionView, loginWithPassword } from "../../lib/api";

interface LoginViewProps {
  onAuthenticated: (session: AuthSessionView) => void;
}

const TENANT_SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,62}$/;

/** Optional `?tenant=<slug>` deep link for users who belong to several organizations. */
function tenantSlugFromUrl(): string | undefined {
  const raw = new URLSearchParams(window.location.search).get("tenant")?.trim().toLowerCase();
  return raw && TENANT_SLUG_PATTERN.test(raw) ? raw : undefined;
}

export function LoginView({ onAuthenticated }: LoginViewProps) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [tenantSlug] = useState(tenantSlugFromUrl);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!email.trim() || !password) {
      setError("Please enter both your email and password.");
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      const session = await loginWithPassword({
        email,
        password,
        tenant_slug: tenantSlug,
      });
      onAuthenticated(session);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unable to sign in.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="min-h-screen flex flex-col bg-[var(--color-bg-muted)] text-[var(--color-text)]">
      {/* 56px MaxiBoard white topbar */}
      <header className="h-14 px-4 sm:px-6 bg-[var(--color-bg)] border-b border-[var(--color-border)] flex items-center justify-between gap-2">
        <div className="flex items-center gap-3 min-w-0">
          <span className="w-7 h-7 rounded-[6px] bg-[var(--color-primary)] text-white font-bold text-sm inline-flex items-center justify-center shrink-0">
            M
          </span>
          <span className="font-bold text-base tracking-tight text-[var(--color-text)] truncate">
            MaxiLoop
          </span>
          <span className="hidden sm:inline text-xs text-[var(--color-text-dim)] truncate">
            Error tracking &amp; performance monitoring
          </span>
        </div>
      </header>

      {/* Centered MaxiBoard Sign-In Card */}
      <main className="flex-1 flex items-center justify-center p-4 sm:p-6">
        <section className="w-full max-w-[440px] bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg p-6 sm:p-8 shadow-[var(--pm-shadow-card)]">
          <div className="w-9 h-9 rounded-[6px] bg-[var(--color-primary)] text-white font-bold text-base inline-flex items-center justify-center mb-4">
            M
          </div>

          <h1 className="text-[22px] font-bold text-[var(--color-text)] leading-tight">
            Sign in to MaxiLoop
          </h1>
          <p className="mt-1 text-sm text-[var(--color-text-muted)]">
            Enter your workspace credentials to continue.
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
              <label
                htmlFor="login-email"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5"
              >
                Email
              </label>
              <div className="relative">
                <Mail
                  size={15}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                />
                <input
                  id="login-email"
                  type="email"
                  autoComplete="email"
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="you@company.com"
                  className="pm-input pl-10 h-10 text-sm"
                />
              </div>
            </div>

            <div>
              <label
                htmlFor="login-password"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5"
              >
                Password
              </label>
              <div className="relative">
                <Lock
                  size={15}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                />
                <input
                  id="login-password"
                  type={showPassword ? "text" : "password"}
                  autoComplete="current-password"
                  required
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="pm-input pl-10 pr-10 h-10 text-sm"
                />
                <button
                  type="button"
                  onClick={() => setShowPassword((prev) => !prev)}
                  aria-label={showPassword ? "Hide password" : "Show password"}
                  aria-pressed={showPassword}
                  title={showPassword ? "Hide password" : "Show password"}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 p-1 rounded text-[var(--color-text-dim)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg-hover)] transition cursor-pointer"
                >
                  {showPassword ? <EyeOff size={15} /> : <Eye size={15} />}
                </button>
              </div>
            </div>

            <button
              type="submit"
              disabled={submitting}
              className="w-full h-10 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-60 text-white font-semibold text-sm transition cursor-pointer"
            >
              {submitting ? "Signing in..." : "Sign in"}
            </button>
          </form>

          <p className="mt-5 text-xs text-[var(--color-text-dim)]">
            Forgot your password or need an account? Ask your workspace administrator.
          </p>
        </section>
      </main>
    </div>
  );
}
