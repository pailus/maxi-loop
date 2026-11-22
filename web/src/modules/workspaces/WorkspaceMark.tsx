import { resolveWorkspaceColor } from "./workspacePrefs";

interface WorkspaceMarkProps {
  name: string;
  slug: string;
  tenantSlug: string;
  colorOverride?: string;
  size?: number;
}

// Square workspace mark (first letter on workspace color), matching MaxiBoard's WorkspaceMark.
export function WorkspaceMark({
  name,
  slug,
  tenantSlug,
  colorOverride,
  size = 32,
}: WorkspaceMarkProps) {
  const color = colorOverride ?? resolveWorkspaceColor(tenantSlug, slug);
  const letter = (name.trim() || slug.trim() || "W")[0].toUpperCase();

  return (
    <span
      className="inline-flex items-center justify-center rounded-md text-white font-bold shrink-0 select-none"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.45),
        backgroundColor: color,
      }}
      aria-hidden="true"
    >
      {letter}
    </span>
  );
}
