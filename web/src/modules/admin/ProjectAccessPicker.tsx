import { useEffect, useState } from "react";
import { type ProjectView, fetchProjects } from "../../lib/api";

interface ProjectAccessPickerProps {
  workspaceSlug: string;
  /** `null` = every project of the workspace; an array = only these project ids. */
  value: number[] | null;
  onChange: (value: number[] | null) => void;
  disabled?: boolean;
}

/** "All projects" or a checklist of the workspace's projects (workspace member/viewer only). */
export function ProjectAccessPicker({
  workspaceSlug,
  value,
  onChange,
  disabled,
}: ProjectAccessPickerProps) {
  const [projects, setProjects] = useState<ProjectView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setProjects(null);
    setError(null);
    fetchProjects(workspaceSlug)
      .then((list) => {
        if (active) setProjects(list);
      })
      .catch((err) => {
        if (active) setError(err instanceof Error ? err.message : "Failed to load projects");
      });
    return () => {
      active = false;
    };
  }, [workspaceSlug]);

  const limited = value !== null;
  const selected = new Set(value ?? []);

  function toggle(id: number) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onChange([...next]);
  }

  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-3 text-xs">
        <label className="inline-flex items-center gap-1.5 cursor-pointer">
          <input
            type="radio"
            checked={!limited}
            disabled={disabled}
            onChange={() => onChange(null)}
          />
          <span>All projects</span>
        </label>
        <label className="inline-flex items-center gap-1.5 cursor-pointer">
          <input
            type="radio"
            checked={limited}
            disabled={disabled}
            onChange={() => onChange([])}
          />
          <span>Selected projects</span>
        </label>
      </div>
      {limited && (
        <div className="max-h-40 overflow-y-auto rounded border border-[var(--color-border)] p-1.5 space-y-0.5">
          {error ? (
            <div className="text-xs text-[var(--color-danger-text)]">{error}</div>
          ) : projects === null ? (
            <div className="text-xs text-[var(--color-text-dim)]">Loading projects...</div>
          ) : projects.length === 0 ? (
            <div className="text-xs text-[var(--color-text-dim)]">
              This workspace has no projects yet.
            </div>
          ) : (
            projects.map((p) => (
              <label
                key={p.id}
                className="flex items-center gap-2 px-1 py-0.5 rounded text-xs hover:bg-[var(--color-bg-hover)] cursor-pointer"
              >
                <input
                  type="checkbox"
                  checked={selected.has(p.id)}
                  disabled={disabled}
                  onChange={() => toggle(p.id)}
                />
                <span className="truncate">{p.name}</span>
                <span className="pm-mono text-[10px] text-[var(--color-text-dim)] ml-auto">
                  {p.slug}
                </span>
              </label>
            ))
          )}
        </div>
      )}
      {limited && selected.size === 0 && (
        <div className="text-[11px] text-[var(--color-warning-text,#a54800)]">
          Select at least one project.
        </div>
      )}
    </div>
  );
}

/** Compact label for a member's or invitation's project access. */
export function ProjectAccessSummary({
  limited,
  projects,
}: {
  limited?: boolean;
  projects?: { id: number; slug: string }[];
}) {
  if (!limited) {
    return <span className="text-[11px] text-[var(--color-text-dim)]">All projects</span>;
  }
  if (!projects || projects.length === 0) {
    return <span className="text-[11px] text-[var(--color-danger-text)]">No projects</span>;
  }
  return (
    <span className="inline-flex flex-wrap gap-1">
      {projects.map((p) => (
        <span key={p.id} className="pm-lozenge pm-mono lowercase">
          {p.slug}
        </span>
      ))}
    </span>
  );
}
