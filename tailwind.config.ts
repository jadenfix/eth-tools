import type { Config } from 'tailwindcss';

// Tailwind v4 auto-detects content; this config exists so editor tooling
// resolves a project root. Add design tokens here as the dashboard grows.
const config: Config = {
  content: ['./app/**/*.{ts,tsx}'],
};

export default config;
