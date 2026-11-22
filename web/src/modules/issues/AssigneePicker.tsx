import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, UserMinus, UserRound } from "lucide-react";
import { colorFor, initials } from "../../lib/avatar";
import {
  type IssueAssigneeView,
  type IssueNoteView,
  type IssueSummaryView,
  assignIssue,
  fetchAssignableMembers,
} from "../../lib/api";

export function AssigneeAvatar({
  assignee,
  size = 20,
}: {
  assignee: IssueAssigneeView;
  size?: number;
}) {
  return (
    <span
      className="rounded-full text-white font-bold inline-flex items-center justify-center shrink-0"
      style={{
        width: size,
        height: size,
        fontSize: Math.max(9, Math.round(size * 0.42)),
        background: colorFor(assignee.email),
      }}
      title={`Assigned to ${assignee.name} (${assignee.email})`}
      aria-label={`Assigned to ${assignee.name}`}
    >
      {initials(assignee.name || assignee.email)}
    </span>
  );
}

interface AssigneePickerProps {
  issue: IssueSummaryView;
  canAssign: boolean;
  currentUserId: number;
  /** `note` is the timeline entry recording the change. */
  onAssigned: (issue: IssueSummaryView, note: IssueNoteView | null) => void;
  onError: (message: string) => void;
}

/** Shows the issue's assignee; triagers can open it to assign, reassign or unassign. */
export function AssigneePicker({
  issue,
  canAssign,
  currentUserId,
  onAssigned,
  onError,
}: AssigneePickerProps) {
  const [open, setOpen] = useState(false);
  const [members, setMembers] = useState<IssueAssigneeView[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const assignee = issue.assignee ?? null;

  // The eligible set depends on the issue's project, so reload when the issue changes.
  useEffect(() => {
    setMembers(null);
    setOpen(false);
    setFilter("");
  }, [issue.id]);

  useEffect(() => {
    if (!open || members !== null) return;
    let active = true;
    fetchAssignableMembers(issue.id)
      .then((list) => {
        if (active) setMembers(list);
      })
      .catch((err) => {
        if (active) {
          setOpen(false);
          onErrorRef.current(err instanceof Error ? err.message : "Failed to load members");
        }
      });
    return () => {
      active = false;
    };
  }, [open, members, issue.id]);

  useEffect(() => {
    if (!open) return;
    function onDocClick(e: MouseEvent) {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDocClick);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  async function choose(userId: number | null) {
    setOpen(false);
    if ((assignee?.user_id ?? null) === userId) return;
    setBusy(true);
    try {
      const res = await assignIssue(issue.id, userId);
      onAssigned(res.issue, res.note);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to assign issue");
    } finally {
      setBusy(false);
    }
  }

  const label = assignee ? assignee.name : "Unassigned";
  const trigger = (
    <span className="inline-flex items-center gap-1.5 min-w-0">
      {assignee ? (
        <AssigneeAvatar assignee={assignee} />
      ) : (
        <span className="w-5 h-5 rounded-full border border-dashed border-[var(--color-border-strong)] inline-flex items-center justify-center text-[var(--color-text-dim)] shrink-0">
          <UserRound size={11} />
        </span>
      )}
      <span className="truncate max-w-[140px]">{label}</span>
    </span>
  );

  if (!canAssign) {
    return (
      <span className="h-7 px-1.5 inline-flex items-center text-xs font-medium text-[var(--color-text-muted)]">
        {trigger}
      </span>
    );
  }

  const meEligible = members?.some((m) => m.user_id === currentUserId) ?? false;
  const q = filter.trim().toLowerCase();
  const shown = (members ?? []).filter(
    (m) => !q || m.name.toLowerCase().includes(q) || m.email.toLowerCase().includes(q),
  );

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        disabled={busy}
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Assignee: ${label}`}
        className="h-7 px-1.5 rounded hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1 cursor-pointer disabled:opacity-50"
      >
        {trigger}
        <ChevronDown size={12} className="shrink-0" />
      </button>

      {open && (
        <div className="absolute z-30 mt-1 left-0 w-64 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-raised)] p-1">
          {members === null ? (
            <div className="px-2 py-3 text-xs text-[var(--color-text-dim)]">Loading members...</div>
          ) : (
            <>
              {members.length > 6 && (
                <input
                  type="text"
                  autoFocus
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  placeholder="Search members..."
                  aria-label="Search members"
                  className="pm-input h-8 text-xs mb-1"
                />
              )}
              {meEligible && assignee?.user_id !== currentUserId && (
                <button
                  type="button"
                  onClick={() => void choose(currentUserId)}
                  className="w-full px-2 py-1.5 rounded text-left text-xs font-semibold text-[var(--color-link)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
                >
                  Assign to me
                </button>
              )}
              {assignee && (
                <button
                  type="button"
                  onClick={() => void choose(null)}
                  className="w-full px-2 py-1.5 rounded text-left text-xs text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] inline-flex items-center gap-1.5 cursor-pointer"
                >
                  <UserMinus size={12} />
                  <span>Unassign</span>
                </button>
              )}
              <div role="listbox" className="max-h-60 overflow-y-auto">
                {shown.length === 0 ? (
                  <div className="px-2 py-2 text-xs text-[var(--color-text-dim)]">
                    No members can be assigned.
                  </div>
                ) : (
                  shown.map((m) => (
                    <button
                      key={m.user_id}
                      type="button"
                      role="option"
                      aria-selected={assignee?.user_id === m.user_id}
                      onClick={() => void choose(m.user_id)}
                      className="w-full px-2 py-1.5 rounded text-left hover:bg-[var(--color-bg-hover)] flex items-center gap-2 cursor-pointer"
                    >
                      <AssigneeAvatar assignee={m} />
                      <span className="min-w-0 flex-1">
                        <span className="block text-xs font-semibold text-[var(--color-text)] truncate">
                          {m.name}
                        </span>
                        <span className="block text-[11px] text-[var(--color-text-dim)] truncate">
                          {m.email}
                        </span>
                      </span>
                      {assignee?.user_id === m.user_id && (
                        <Check size={13} className="text-[var(--color-primary)] shrink-0" />
                      )}
                    </button>
                  ))
                )}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
