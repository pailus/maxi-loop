import { useMemo, useState } from "react";
import { ChevronRight, Folder } from "lucide-react";
import type { ProjectView } from "../../lib/api";
import { ProjectIcon } from "./ProjectIcon";

const COLLAPSED_KEY = "ml_project_groups_collapsed";

function useCollapsedGroups(): [Set<string>, (tag: string) => void] {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    try {
      const raw = localStorage.getItem(COLLAPSED_KEY);
      return new Set(raw ? (JSON.parse(raw) as string[]) : []);
    } catch {
      return new Set();
    }
  });

  const toggle = (tag: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(tag)) next.delete(tag);
      else next.add(tag);
      try {
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
      } catch {
        // Ignore storage quota errors
      }
      return next;
    });
  };

  return [collapsed, toggle];
}

interface ProjectTreeProps {
  projects: ProjectView[];
  selectedProjectSlug: string;
  highlightSelection: boolean;
  onSelectProject: (slug: string) => void;
}

/**
 * MaxiBoard-style collapsible project tree grouped by project tags (`BoardTree` pattern):
 * - Tag folders appear first (alphabetically) with chevron + Folder icon + project count
 * - Untagged projects appear cleanly below folders at root level
 */
export function ProjectTree({
  projects,
  selectedProjectSlug,
  highlightSelection,
  onSelectProject,
}: ProjectTreeProps) {
  const [collapsed, toggle] = useCollapsedGroups();

  const { groups, ungrouped } = useMemo(() => {
    const byTag = new Map<string, ProjectView[]>();
    const root: ProjectView[] = [];

    for (const p of projects) {
      const tags = Array.isArray(p.tags) ? p.tags : [];
      if (tags.length === 0) {
        root.push(p);
      } else {
        for (const tag of tags) {
          const list = byTag.get(tag) ?? [];
          list.push(p);
          byTag.set(tag, list);
        }
      }
    }

    const sortedGroups = [...byTag.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([tag, items]) => ({
        tag,
        items: items.sort((a, b) => a.slug.localeCompare(b.slug)),
      }));

    root.sort((a, b) => a.slug.localeCompare(b.slug));
    return { groups: sortedGroups, ungrouped: root };
  }, [projects]);

  const hasGroups = groups.length > 0;

  const renderProjectRow = (p: ProjectView, indented: boolean, keyPrefix: string) => {
    const isActive = highlightSelection && selectedProjectSlug === p.slug;
    return (
      <li key={`${keyPrefix}:${p.id}`}>
        <button
          type="button"
          onClick={() => onSelectProject(p.slug)}
          style={indented ? { paddingLeft: 28 } : undefined}
          className={`w-full min-h-11 md:min-h-8 ${indented ? "pr-2" : "px-2.5"} rounded-md flex items-center gap-2.5 text-sm cursor-pointer transition ${
            isActive
              ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold"
              : "text-[var(--color-text)] hover:bg-[var(--color-bg-hover)]"
          }`}
          title={p.tags.length > 0 ? `${p.name} (${p.tags.join(", ")})` : p.name}
        >
          <ProjectIcon slug={p.slug} platform={p.platform} size={20} />
          <span className="truncate flex-1 text-left">{p.slug}</span>
        </button>
      </li>
    );
  };

  return (
    <ul className="space-y-0.5">
      {groups.map(({ tag, items }) => {
        const isOpen = !collapsed.has(tag);
        const containsActive =
          highlightSelection &&
          items.some((p) => p.slug === selectedProjectSlug);
        return (
          <li key={`group:${tag}`}>
            <button
              type="button"
              onClick={() => toggle(tag)}
              aria-expanded={isOpen}
              className={`w-full min-h-11 md:min-h-8 px-2 rounded-md flex items-center gap-1.5 text-sm font-semibold cursor-pointer transition hover:bg-[var(--color-bg-hover)] ${
                containsActive && !isOpen
                  ? "text-[var(--color-primary)] bg-[var(--color-primary-light)]"
                  : "text-[var(--color-text)]"
              }`}
            >
              <ChevronRight
                size={14}
                className={`shrink-0 text-[var(--color-text-dim)] transition-transform duration-150 ${
                  isOpen ? "rotate-90" : ""
                }`}
              />
              <Folder
                size={15}
                strokeWidth={1.75}
                className="shrink-0 text-[var(--color-text-muted)]"
              />
              <span className="truncate flex-1 text-left">{tag}</span>
              <span className="min-w-5 h-5 px-1.5 rounded-full bg-[var(--pm-col)] text-[var(--color-text-muted)] text-[11px] font-semibold leading-5 text-center">
                {items.length}
              </span>
            </button>
            {isOpen && (
              <ul className="mt-0.5 space-y-0.5">
                {items.map((p) => renderProjectRow(p, true, `g-${tag}`))}
              </ul>
            )}
          </li>
        );
      })}

      {ungrouped.map((p) => renderProjectRow(p, false, hasGroups ? "root" : "flat"))}
    </ul>
  );
}
