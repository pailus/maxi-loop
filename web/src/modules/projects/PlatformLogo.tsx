import { Code2 } from "lucide-react";
import { ONBOARDING_PLATFORMS } from "./onboardingPlatforms";
import { PLATFORM_LOGOS } from "./platformLogos";

const NAVY = "#172B4D";

/** Rounded tile with the official stack logo, tinted with the brand color. */
export function PlatformLogo({ platform, size = 36 }: { platform: string; size?: number }) {
  const logo = PLATFORM_LOGOS[platform];
  const color = logo?.color ?? "#626F86";
  const iconSize = Math.round(size * 0.58);

  return (
    <span
      aria-hidden="true"
      className="shrink-0 rounded-lg inline-flex items-center justify-center border"
      style={{
        width: size,
        height: size,
        background: `color-mix(in srgb, ${color} 9%, #ffffff)`,
        borderColor: `color-mix(in srgb, ${color} 22%, #ffffff)`,
      }}
    >
      {logo ? (
        <svg
          viewBox="0 0 24 24"
          width={iconSize}
          height={iconSize}
          xmlns="http://www.w3.org/2000/svg"
          focusable="false"
        >
          {/* The JavaScript mark is a yellow square with knocked-out letters: back it with navy. */}
          {platform === "javascript" && <rect x="3" y="3" width="18" height="18" fill={NAVY} />}
          <path d={logo.path} fill={color} />
        </svg>
      ) : (
        <Code2 size={iconSize} color={color} strokeWidth={2.2} />
      )}
    </span>
  );
}

/** Compact chip (logo + platform name) for project cards and lists. */
export function PlatformChip({ platform }: { platform: string }) {
  const logo = PLATFORM_LOGOS[platform];
  const known = ONBOARDING_PLATFORMS.find((p) => p.id === platform);
  const label = known?.label ?? platform;
  const color = logo?.color ?? "#626F86";

  return (
    <span
      className="inline-flex items-center gap-1 h-5 pl-1 pr-1.5 rounded border text-[11px] font-semibold text-[var(--color-text)]"
      style={{
        background: `color-mix(in srgb, ${color} 8%, #ffffff)`,
        borderColor: `color-mix(in srgb, ${color} 22%, #ffffff)`,
      }}
      title={`Platform: ${platform}`}
    >
      {logo ? (
        <svg viewBox="0 0 24 24" width={13} height={13} aria-hidden="true" focusable="false">
          {platform === "javascript" && <rect x="3" y="3" width="18" height="18" fill={NAVY} />}
          <path d={logo.path} fill={color} />
        </svg>
      ) : (
        <Code2 size={12} color={color} strokeWidth={2.4} aria-hidden="true" />
      )}
      <span>{label}</span>
    </span>
  );
}
