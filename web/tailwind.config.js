import plugin from "tailwindcss/plugin";

// Original Graine dashboard palette (src/dashboard/style.css).
// Keep all theme colours here rather than scattering literal colours through components.
// Values are exposed as both direct CSS variables (for charts) and RGB channels (for Tailwind).
const light = {
  background: "#f8f8f6",
  foreground: "#20231e",
  card: "#ffffff", "card-foreground": "#20231e",
  popover: "#ffffff", "popover-foreground": "#20231e",
  primary: "#526b43", "primary-foreground": "#ffffff",
  secondary: "#f2f3ef", "secondary-foreground": "#20231e",
  muted: "#f2f3ef", "muted-foreground": "#73786f",
  accent: "#e8eee2", "accent-foreground": "#20231e",
  destructive: "#a64f43", "destructive-foreground": "#ffffff",
  border: "#e3e5df", input: "#e3e5df", ring: "#526b43",
  "chart-radarr": "#b17d4b",
  "chart-sonarr": "#6c91c5",
  "chart-autobrr": "#6d9b67",
  "chart-other": "#92978d",
  "chart-low": "#966721",
  "series-1": "#6d9b67",
  "series-2": "#6c91c5",
  "series-3": "#b17d4b",
  "series-4": "#a678ae",
  "series-5": "#bd6872",
  "series-6": "#5d9c9a",
  "series-7": "#9a8c58",
  "series-8": "#8a8f9d",
};

const dark = {
  background: "#151714",
  foreground: "#e8ebe5",
  card: "#1d201c", "card-foreground": "#e8ebe5",
  popover: "#1d201c", "popover-foreground": "#e8ebe5",
  primary: "#a8c18f", "primary-foreground": "#151714",
  secondary: "#252923", "secondary-foreground": "#e8ebe5",
  muted: "#252923", "muted-foreground": "#a0a69b",
  accent: "#2a3326", "accent-foreground": "#e8ebe5",
  destructive: "#e18a7e", "destructive-foreground": "#151714",
  border: "#333830", input: "#333830", ring: "#a8c18f",
  "chart-radarr": "#d19a65",
  "chart-sonarr": "#83a9df",
  "chart-autobrr": "#8dbb83",
  "chart-other": "#8c9386",
  "chart-low": "#e0b66c",
  "series-1": "#8dbb83",
  "series-2": "#83a9df",
  "series-3": "#d19a65",
  "series-4": "#bd91c7",
  "series-5": "#d47d88",
  "series-6": "#78b6b3",
  "series-7": "#c1b477",
  "series-8": "#a2a8b8",
};

function themeVars(palette) {
  const result = {};
  for (const [name, hex] of Object.entries(palette)) {
    const raw = hex.replace("#", "");
    const value = raw.length === 3 ? [...raw].map((ch) => ch + ch).join("") : raw;
    if (!/^[0-9a-f]{6}$/i.test(value)) throw new Error(`Invalid palette colour: ${name}=${hex}`);
    result[`--${name}`] = hex;
    result[`--${name}-rgb`] = [0, 2, 4].map((i) => parseInt(value.slice(i, i + 2), 16)).join(" ");
  }
  return result;
}

const semanticColor = (name) => `rgb(var(--${name}-rgb) / <alpha-value>)`;

/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    container: { center: true, padding: "1.5rem", screens: { "2xl": "1440px" } },
    extend: {
      colors: {
        border: semanticColor("border"), input: semanticColor("input"),
        ring: semanticColor("ring"), background: semanticColor("background"),
        foreground: semanticColor("foreground"),
        primary: { DEFAULT: semanticColor("primary"), foreground: semanticColor("primary-foreground") },
        secondary: { DEFAULT: semanticColor("secondary"), foreground: semanticColor("secondary-foreground") },
        muted: { DEFAULT: semanticColor("muted"), foreground: semanticColor("muted-foreground") },
        accent: { DEFAULT: semanticColor("accent"), foreground: semanticColor("accent-foreground") },
        destructive: { DEFAULT: semanticColor("destructive"), foreground: semanticColor("destructive-foreground") },
        card: { DEFAULT: semanticColor("card"), foreground: semanticColor("card-foreground") },
        popover: { DEFAULT: semanticColor("popover"), foreground: semanticColor("popover-foreground") },
      },
      borderRadius: {
        lg: "var(--radius)", md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      fontFamily: {
        sans: ["system-ui", "-apple-system", "BlinkMacSystemFont", "Segoe UI", "sans-serif"],
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "monospace"],
      },
      boxShadow: { panel: "0 1px 2px rgb(0 0 0 / 0.04)" },
    },
  },
  plugins: [plugin(({ addBase }) => addBase({
    ":root": { ...themeVars(light), "--radius": "0.75rem", "color-scheme": "light" },
    ".dark": { ...themeVars(dark), "color-scheme": "dark" },
  }))],
};
