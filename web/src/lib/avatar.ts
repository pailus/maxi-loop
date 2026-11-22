// Avatar helpers ported from MaxiBoard (maxi-code static/src/pm/shell/initials.js)
// so user avatars get the same initials and colour in both apps.

const AVATAR_COLORS = [
  "#1D7AFC",
  "#22A06B",
  "#6E5DC6",
  "#E56910",
  "#C9372C",
  "#0B7285",
  "#8F7EE7",
];

export function initials(name = ""): string {
  const clean = String(name).split("@")[0].replace(/[._-]+/g, " ").trim();
  const parts = clean.split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

export function colorFor(key = ""): string {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}
