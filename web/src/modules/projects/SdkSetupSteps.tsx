import { useState } from "react";
import { Check, Copy } from "lucide-react";
import type { SdkSnippetStep } from "./onboardingPlatforms";

export function CodeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative group">
      <pre className="p-3 pr-16 rounded-md bg-[var(--pm-col)] border border-[var(--color-border)] pm-mono text-xs leading-relaxed text-[var(--color-text)] overflow-x-auto whitespace-pre">
        {code}
      </pre>
      <button
        type="button"
        onClick={() => {
          navigator.clipboard?.writeText(code).catch(() => {});
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        }}
        className="absolute top-1.5 right-1.5 h-6 px-1.5 rounded text-[10px] font-semibold inline-flex items-center gap-1 bg-[var(--color-bg-card)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
      >
        {copied ? <Check size={11} /> : <Copy size={11} />}
        <span>{copied ? "Copied" : "Copy"}</span>
      </button>
    </div>
  );
}

/** Numbered Install → Configure → Verify list of copyable SDK snippets. */
export function SdkSetupSteps({ steps }: { steps: SdkSnippetStep[] }) {
  return (
    <ol className="space-y-3">
      {steps.map((s, idx) => (
        <li key={s.title} className="flex gap-2.5">
          <span className="w-5 h-5 mt-0.5 rounded-full bg-[var(--color-bg-muted)] border border-[var(--color-border)] text-[10px] font-bold text-[var(--color-text-muted)] inline-flex items-center justify-center shrink-0">
            {idx + 1}
          </span>
          <div className="flex-1 min-w-0 space-y-1">
            <p className="text-xs font-semibold text-[var(--color-text)]">{s.title}</p>
            {s.description && (
              <p className="text-[11px] text-[var(--color-text-dim)]">{s.description}</p>
            )}
            <CodeBlock code={s.code} />
          </div>
        </li>
      ))}
    </ol>
  );
}
