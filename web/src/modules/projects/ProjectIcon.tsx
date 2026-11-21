import type { CSSProperties, ReactNode } from "react";

export type ProjectIconId =
  | "software-development"
  | "web-development"
  | "cloud-devops"
  | "cyber-security"
  | "data-analytics"
  | "ai-automation"
  | "mobile-app"
  | "ecommerce"
  | "finance"
  | "infrastructure-iot"
  | "project-management"
  | "ui-ux-design";

export interface ProjectIconOption {
  id: ProjectIconId;
  label: string;
  defaultColor: string;
}

export const PROJECT_ICON_OPTIONS: ProjectIconOption[] = [
  { id: "software-development", label: "Software & API", defaultColor: "#0C66E4" },
  { id: "web-development", label: "Web & Frontend", defaultColor: "#22A06B" },
  { id: "cloud-devops", label: "Cloud & Workers", defaultColor: "#6E5DC6" },
  { id: "infrastructure-iot", label: "Systems & Rust/Go", defaultColor: "#D97008" },
  { id: "cyber-security", label: "Auth & Security", defaultColor: "#C9372C" },
  { id: "finance", label: "Billing & Payments", defaultColor: "#0C66E4" },
  { id: "ecommerce", label: "Storefront & Cart", defaultColor: "#22A06B" },
  { id: "data-analytics", label: "Analytics & Telemetry", defaultColor: "#6E5DC6" },
  { id: "ai-automation", label: "AI & MCP Agent", defaultColor: "#1D7AFC" },
  { id: "mobile-app", label: "Mobile Client", defaultColor: "#22A06B" },
  { id: "project-management", label: "Core Platform", defaultColor: "#0C66E4" },
  { id: "ui-ux-design", label: "Design & UI", defaultColor: "#6E5DC6" },
];

export const PROJECT_SWATCH_COLORS = [
  "#0C66E4",
  "#22A06B",
  "#6E5DC6",
  "#D97008",
  "#C9372C",
  "#206A83",
];

const CUSTOM_ICON_STORAGE_KEY = "maxiloop_project_icons_v1";

interface StoredProjectVisual {
  icon: ProjectIconId;
  color: string;
}

function readStoredVisuals(): Record<string, StoredProjectVisual> {
  try {
    const raw = localStorage.getItem(CUSTOM_ICON_STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as Record<string, StoredProjectVisual>;
  } catch {
    return {};
  }
}

export function saveProjectVisual(
  projectSlug: string,
  icon: ProjectIconId,
  color: string,
): void {
  try {
    const existing = readStoredVisuals();
    existing[projectSlug] = { icon, color };
    localStorage.setItem(CUSTOM_ICON_STORAGE_KEY, JSON.stringify(existing));
  } catch {
    // Ignore storage quota errors
  }
}

export function resolveProjectVisual(project: {
  slug: string;
  platform?: string;
}): StoredProjectVisual {
  const stored = readStoredVisuals()[project.slug];
  if (stored) {
    return stored;
  }

  const slug = project.slug.toLowerCase();
  const platform = (project.platform ?? "").toLowerCase();

  if (slug.includes("bill") || slug.includes("pay") || slug.includes("fin")) {
    return { icon: "finance", color: "#6E5DC6" };
  }
  if (slug.includes("store") || slug.includes("shop") || slug.includes("cart")) {
    return { icon: "ecommerce", color: "#22A06B" };
  }
  if (slug.includes("auth") || slug.includes("sec") || slug.includes("iam")) {
    return { icon: "cyber-security", color: "#C9372C" };
  }
  if (slug.includes("ai") || slug.includes("mcp") || slug.includes("agent")) {
    return { icon: "ai-automation", color: "#0C66E4" };
  }
  if (
    platform.includes("react") ||
    platform.includes("javascript") ||
    platform.includes("typescript") ||
    slug.includes("web") ||
    slug.includes("ui")
  ) {
    return { icon: "web-development", color: "#22A06B" };
  }
  if (platform.includes("rust") || platform.includes("go")) {
    return { icon: "infrastructure-iot", color: "#6E5DC6" };
  }
  if (platform.includes("cloud") || slug.includes("worker")) {
    return { icon: "cloud-devops", color: "#6E5DC6" };
  }
  return { icon: "software-development", color: "#0C66E4" };
}

function renderIconInner(icon: ProjectIconId): ReactNode {
  switch (icon) {
    case "web-development":
      return (
        <>
          <rect
            x="26"
            y="52"
            width="80"
            height="53"
            rx="11"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="6.5"
            transform="rotate(-3 66 79)"
          />
          <path d="M26 65 Q61 63 105 63" stroke="#112955" strokeWidth="6" fill="none" />
          <circle cx="36" cy="59" r="2.9" fill="#FF902C" />
          <circle cx="46" cy="59" r="2.9" fill="#FFD24B" />
          <path
            d="M51 77 l-9 7 9 7 M80 77 l9 7 -9 7 M72 73 l-11 24"
            fill="none"
            stroke="#112955"
            strokeWidth="6.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <path d="M95 93 l5 -3 3 5 -5 3Z" fill="#FFD24B" stroke="#112955" strokeWidth="2.6" />
        </>
      );
    case "cloud-devops":
      return (
        <>
          <path
            d="M42 98 C28 98 24 87 30 78 Q35 70 44 72 C45 59 58 52 68 59 Q74 62 76 69 C87 61 102 71 99 81 Q111 83 107 94 Q104 102 92 102 L44 102Z"
            fill="#379FF5"
            stroke="#112955"
            strokeWidth="6.5"
            strokeLinejoin="round"
          />
          <path
            d="M59 73 l-11 12 11 0 M75 95 l11 -12 -11 0"
            fill="none"
            stroke="#FFF9F1"
            strokeWidth="5.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <path d="M66 78 l0 18" stroke="#FFD24B" strokeWidth="6" strokeLinecap="round" />
          <path d="M27 61 l5 -4 M94 54 l6 5" stroke="#FF902C" strokeWidth="4" strokeLinecap="round" />
        </>
      );
    case "cyber-security":
      return (
        <>
          <path
            d="M66 50 L95 59 L93 85 Q87 102 66 111 Q45 102 39 85 L37 59 Z"
            fill="#379FF5"
            stroke="#112955"
            strokeWidth="7"
            strokeLinejoin="round"
          />
          <path
            d="M55 81 v-8 a11 11 0 0 1 22 0 v8"
            fill="none"
            stroke="#112955"
            strokeWidth="5.5"
            strokeLinecap="round"
          />
          <rect x="52" y="79" width="28" height="22" rx="6" fill="#FFD24B" stroke="#112955" strokeWidth="5" />
          <circle cx="66" cy="89" r="3" fill="#112955" />
          <path d="M98 52 l4 -7 M104 61 h5" stroke="#FF902C" strokeWidth="4.5" strokeLinecap="round" />
        </>
      );
    case "data-analytics":
      return (
        <>
          <path d="M29 105 L29 61" stroke="#112955" strokeWidth="6.5" strokeLinecap="round" />
          <rect x="37" y="83" width="15" height="22" rx="5" fill="#56D6C8" stroke="#112955" strokeWidth="5" />
          <rect x="58" y="70" width="15" height="35" rx="5" fill="#379FF5" stroke="#112955" strokeWidth="5" />
          <rect x="79" y="57" width="15" height="48" rx="5" fill="#FFD24B" stroke="#112955" strokeWidth="5" />
          <path
            d="M30 74 Q49 62 62 65 T98 49"
            stroke="#FF902C"
            strokeWidth="5"
            fill="none"
            strokeLinecap="round"
          />
          <path
            d="M90 49 L100 46 L98 56"
            fill="none"
            stroke="#FF902C"
            strokeWidth="4.5"
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        </>
      );
    case "ai-automation":
      return (
        <>
          <rect
            x="35"
            y="61"
            width="64"
            height="44"
            rx="17"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="7"
            transform="rotate(-3 67 83)"
          />
          <rect x="44" y="71" width="46" height="24" rx="10" fill="#FFF9F1" stroke="#112955" strokeWidth="4" />
          <circle cx="56" cy="82" r="4.2" fill="#112955" />
          <circle cx="77" cy="82" r="4.2" fill="#112955" />
          <path d="M63 90 q5 5 10 0" fill="none" stroke="#379FF5" strokeWidth="3.3" strokeLinecap="round" />
          <path d="M67 61 v-9" stroke="#112955" strokeWidth="5.5" strokeLinecap="round" />
          <circle cx="67" cy="49" r="5.5" fill="#FFD24B" stroke="#112955" strokeWidth="3.6" />
          <path d="M25 74 v18 M109 73 v18" stroke="#112955" strokeWidth="5" strokeLinecap="round" />
          <circle cx="25" cy="74" r="4.2" fill="#FF902C" />
          <circle cx="109" cy="73" r="4.2" fill="#FF902C" />
        </>
      );
    case "mobile-app":
      return (
        <>
          <rect
            x="39"
            y="49"
            width="48"
            height="63"
            rx="12"
            fill="#379FF5"
            stroke="#112955"
            strokeWidth="7"
            transform="rotate(7 63 80)"
          />
          <path
            d="M50 61 L76 64 L72 96 L46 93 Z"
            fill="#FFF9F1"
            stroke="#112955"
            strokeWidth="4"
            strokeLinejoin="round"
          />
          <circle cx="62" cy="102" r="3.5" fill="#FFD24B" stroke="#112955" strokeWidth="2.6" />
          <path
            d="M96 62 l3 8 8 2 -7 5 -1 8 -6 -6 -8 2 4 -7 -3 -7Z"
            fill="#FFD24B"
            stroke="#112955"
            strokeWidth="3.2"
            strokeLinejoin="round"
          />
          <path
            d="M55 76 q8 -8 17 0 M58 85 l10 1"
            fill="none"
            stroke="#56D6C8"
            strokeWidth="5"
            strokeLinecap="round"
          />
        </>
      );
    case "ecommerce":
      return (
        <>
          <path
            d="M32 60 l8 3 10 35 h42"
            fill="none"
            stroke="#112955"
            strokeWidth="6.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <path
            d="M44 68 L103 67 L93 90 L51 93Z"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="6"
            strokeLinejoin="round"
          />
          <path
            d="M61 69 Q64 81 78 81 Q90 79 91 68"
            fill="none"
            stroke="#379FF5"
            strokeWidth="5"
            strokeLinecap="round"
          />
          <circle cx="57" cy="106" r="6" fill="#FFD24B" stroke="#112955" strokeWidth="4" />
          <circle cx="89" cy="106" r="6" fill="#FFD24B" stroke="#112955" strokeWidth="4" />
          <path d="M68 52 v9 M63 57 h10" stroke="#FF902C" strokeWidth="4" strokeLinecap="round" />
        </>
      );
    case "finance":
      return (
        <>
          <rect
            x="28"
            y="67"
            width="73"
            height="40"
            rx="11"
            fill="#379FF5"
            stroke="#112955"
            strokeWidth="7"
            transform="rotate(-2 65 87)"
          />
          <path
            d="M29 68 L31 57 Q31 52 38 54 L90 58 Q96 58 96 67"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="5.5"
            strokeLinejoin="round"
          />
          <rect x="73" y="76" width="35" height="21" rx="7" fill="#FFD24B" stroke="#112955" strokeWidth="5" />
          <circle cx="90" cy="87" r="4" fill="#112955" />
          <path d="M42 81 l15 -1" stroke="#FFF9F1" strokeWidth="6" strokeLinecap="round" />
          <path d="M101 55 q6 -2 6 4" fill="none" stroke="#FF902C" strokeWidth="4" strokeLinecap="round" />
        </>
      );
    case "infrastructure-iot":
      return (
        <>
          <rect
            x="40"
            y="59"
            width="52"
            height="48"
            rx="10"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="6.5"
            transform="rotate(-2 66 83)"
          />
          <rect x="51" y="70" width="30" height="26" rx="6" fill="#379FF5" stroke="#112955" strokeWidth="4.5" />
          <path
            d="M48 52 v9 M64 52 v8 M80 52 v9 M48 107 v8 M64 107 v8 M80 107 v8 M33 70 h8 M33 86 h8 M93 70 h8 M93 86 h8"
            stroke="#112955"
            strokeWidth="4.5"
            strokeLinecap="round"
          />
          <path d="M61 82 l5 -6 5 6 -5 6Z" fill="#FFD24B" stroke="#112955" strokeWidth="3" />
          <circle cx="100" cy="52" r="5" fill="#FF902C" stroke="#112955" strokeWidth="3.5" />
        </>
      );
    case "project-management":
      return (
        <>
          <path
            d="M24 95 C38 77 51 110 65 92 C79 74 53 79 62 65 C70 55 79 82 89 75 L101 65"
            fill="none"
            stroke="#112955"
            strokeWidth="7.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <circle cx="32" cy="91" r="8.5" fill="#FFD24B" stroke="#112955" strokeWidth="6" />
          <path
            d="M93 63 L109 58 L104 76 Z"
            fill="#FF902C"
            stroke="#112955"
            strokeWidth="5.4"
            strokeLinejoin="round"
          />
          <path
            d="M81 52 l3 -6 3 6 6 3 -6 3 -3 6 -3 -6 -6 -3 Z"
            fill="#FFD24B"
            stroke="#112955"
            strokeWidth="3.3"
            strokeLinejoin="round"
          />
        </>
      );
    case "ui-ux-design":
      return (
        <>
          <path
            d="M28 63 Q26 56 34 55 L78 50 Q85 49 86 57 L89 92 Q90 99 83 100 L36 103 Q29 103 29 96Z"
            fill="#379FF5"
            stroke="#112955"
            strokeWidth="6.4"
          />
          <path d="M43 65 h30 M43 77 h16" fill="none" stroke="#FFF9F1" strokeWidth="6" strokeLinecap="round" />
          <path
            d="M75 100 L94 68 Q97 64 102 68 L107 72 Q110 76 106 81 L85 109 L75 111 Z"
            fill="#FFD24B"
            stroke="#112955"
            strokeWidth="5.5"
            strokeLinejoin="round"
          />
          <path d="M75 100 L84 107 L75 111Z" fill="#FF902C" stroke="#112955" strokeWidth="3.4" />
          <path d="M95 68 l10 11" stroke="#112955" strokeWidth="3.8" />
        </>
      );
    case "software-development":
    default:
      return (
        <>
          <path
            d="M49 57 Q39 55 39 67 L39 72 Q39 79 30 80 Q39 81 39 88 L39 95 Q39 106 49 104"
            stroke="#112955"
            strokeWidth="7.5"
            fill="none"
            strokeLinecap="round"
          />
          <path
            d="M81 57 Q91 55 91 67 L91 72 Q91 79 100 80 Q91 81 91 88 L91 95 Q91 106 81 104"
            stroke="#112955"
            strokeWidth="7.5"
            fill="none"
            strokeLinecap="round"
          />
          <path d="M60 89 l10 -21" stroke="#FF902C" strokeWidth="8" strokeLinecap="round" />
          <circle cx="56" cy="68" r="6" fill="#56D6C8" stroke="#112955" strokeWidth="3.5" />
          <circle cx="73" cy="97" r="6" fill="#FFD24B" stroke="#112955" strokeWidth="3.5" />
        </>
      );
  }
}

export function ProjectIcon({
  slug,
  platform,
  iconOverride,
  colorOverride,
  size = 20,
  className = "",
}: {
  slug: string;
  platform?: string;
  iconOverride?: ProjectIconId;
  colorOverride?: string;
  size?: number;
  className?: string;
}) {
  const resolved = resolveProjectVisual({ slug, platform });
  const icon = iconOverride ?? resolved.icon;
  const color = colorOverride ?? resolved.color;

  const style: CSSProperties & { "--bi-fill"?: string } = {
    width: size,
    height: size,
    "--bi-fill": `${color}3D`,
  };

  return (
    <span className={`pm-bicon shrink-0 ${className}`} style={style} aria-hidden="true">
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" focusable="false">
        <g strokeLinecap="round" strokeLinejoin="round">
          <path
            className="bi-card"
            d="M17 34 C18 24 27 19 38 19 L88 16 C95 15 99 17 105 23 L115 33 C119 37 120 42 119 49 L117 99 C116 112 109 116 96 117 L35 120 C22 120 13 113 12 100 L12 53 C11 45 13 39 17 34 Z"
            stroke="#112955"
            strokeWidth="6.6"
            strokeLinejoin="round"
          />
          <path
            d="M91.5 17 L91 29 C91 37 96 42 105 42 L119 42 L105 23 Q99 17 91.5 17 Z"
            fill="#56D6C8"
            stroke="#112955"
            strokeWidth="6.5"
            strokeLinejoin="round"
          />
          <path
            d="M26 34 Q29 32 32 32 L63 31"
            fill="none"
            stroke="#379FF5"
            strokeWidth="7"
            strokeLinecap="round"
          />
          <path
            d="M75 30 l2 -4 M81 35 l4 -3"
            fill="none"
            stroke="#112955"
            strokeWidth="3.2"
            strokeLinecap="round"
          />
          {renderIconInner(icon)}
        </g>
      </svg>
    </span>
  );
}
