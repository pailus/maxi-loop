import { type FormEvent, useState } from "react";
import {
  AlertCircle,
  Building2,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Lock,
  Mail,
  Terminal,
} from "lucide-react";
import {
  type AuthSessionView,
  type HealthPayload,
  loginWithPassword,
} from "../../lib/api";

interface LoginViewProps {
  health: HealthPayload | null;
  onAuthenticated: (session: AuthSessionView) => void;
}

export function LoginView({ health, onAuthenticated }: LoginViewProps) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [tenantSlug, setTenantSlug] = useState("");
  const [showTenantInput, setShowTenantInput] = useState(false);
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
        tenant_slug: showTenantInput && tenantSlug.trim() ? tenantSlug.trim() : undefined,
      });
      onAuthenticated(session);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unable to sign in.");
    } finally {
      setSubmitting(false);
    }
  }

  const dbHealthy = health?.database === "ok" && health?.status === "ok";

  return (
    <div className="min-h-screen flex flex-col bg-[var(--color-bg-muted)] text-[var(--color-text)]">
      {/* 56px MaxiBoard white topbar */}
      <header className="h-14 px-6 bg-[var(--color-bg)] border-b border-[var(--color-border)] flex items-center justify-between">
        <div className="flex items-center gap-3">
          <span className="w-7 h-7 rounded-[6px] bg-[var(--color-primary)] text-white font-bold text-sm inline-flex items-center justify-center">
            M
          </span>
          <span className="font-bold text-base tracking-tight text-[var(--color-text)]">
            MaxiLoop
          </span>
          <span className="hidden sm:inline text-xs text-[var(--color-text-dim)]">
            Error tracking &amp; performance monitoring
          </span>
        </div>

        <div className="flex items-center gap-2">
          <span
            className={`pm-lozenge ${
              dbHealthy ? "pm-lozenge-success" : "pm-lozenge-warning"
            }`}
          >
            {dbHealthy ? "POSTGRES 16 OK" : "CHECKING DB"}
          </span>
          <span className="pm-lozenge">v{health?.version ?? "0.1.0"}</span>
        </div>
      </header>

      {/* Centered MaxiBoard Sign-In Card */}
      <main className="flex-1 flex items-center justify-center p-6">
        <section className="w-full max-w-[440px] bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg p-8 shadow-[var(--pm-shadow-card)]">
          <div className="w-9 h-9 rounded-[6px] bg-[var(--color-primary)] text-white font-bold text-base inline-flex items-center justify-center mb-4">
            M
          </div>

          <h1 className="text-[22px] font-bold text-[var(--color-text)] leading-tight">
            Sign in to MaxiLoop
          </h1>
          <p className="mt-1 text-sm text-[var(--color-text-muted)]">
            Enter your workspace credentials to continue.
          </p>

          {/* Single-tenant workspace notice */}
          <div className="mt-4 p-2.5 rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] flex items-center gap-2.5">
            <span className="w-5 h-5 rounded bg-[var(--color-success)] text-white font-bold text-[11px] inline-flex items-center justify-center shrink-0">
              D
            </span>
            <div className="min-w-0">
              <div className="text-xs font-semibold text-[var(--color-text)]">
                Default Workspace
              </div>
              <div className="text-[11px] text-[var(--color-text-dim)] truncate">
                Single-tenant default · tenant/workspace chrome hidden
              </div>
            </div>
          </div>

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
                  placeholder="admin@example.com"
                  className="pm-input pl-9 h-10 text-sm"
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
                  type="password"
                  autoComplete="current-password"
                  required
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="••••••••••••"
                  className="pm-input pl-9 h-10 text-sm"
                />
              </div>
            </div>

            {/* Optional multi-tenant organization slug disclosure */}
            <div>
              <button
                type="button"
                onClick={() => setShowTenantInput((prev) => !prev)}
                className="inline-flex items-center gap-1 text-xs font-medium text-[var(--color-text-dim)] hover:text-[var(--color-text)] cursor-pointer"
              >
                {showTenantInput ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <span>Specific tenant slug (optional)</span>
              </button>
              {showTenantInput && (
                <div className="mt-2 relative">
                  <Building2
                    size={15}
                    className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
                  />
                  <input
                    id="login-tenant"
                    type="text"
                    value={tenantSlug}
                    onChange={(e) => setTenantSlug(e.target.value)}
                    placeholder="default"
                    className="pm-input pm-mono pl-9 h-9 text-xs"
                  />
                </div>
              )}
            </div>

            <button
              type="submit"
              disabled={submitting}
              className="w-full h-10 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-60 text-white font-semibold text-sm transition cursor-pointer"
            >
              {submitting ? "Signing in..." : "Sign in"}
            </button>
          </form>

          <hr className="my-5 border-[var(--color-border)]" />

          <div className="space-y-2">
            <div className="flex items-center gap-1.5 text-xs font-semibold text-[var(--color-text-muted)]">
              <Terminal size={13} />
              <span>First-boot or password reset</span>
            </div>
            <div className="px-3 py-2 rounded bg-[var(--pm-col)] text-xs pm-mono text-[var(--color-text)]">
              $ maxi-loop bootstrap
            </div>
            <div className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-dim)] pt-1">
              <CheckCircle2 size={12} className="text-[var(--color-success)] shrink-0" />
              <span>HttpOnly session cookie · Argon2id · CSRF token enforced</span>
            </div>
          </div>
        </section>
      </main>
    </div>
  );
}
