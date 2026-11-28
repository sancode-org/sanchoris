import { resolve } from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig, type ViteUserConfig } from "vite-plus"

export default defineConfig({
  root: resolve(import.meta.dirname, "apps/frontend"),
  fmt: {
    semi: false,
    singleQuote: false,
    trailingComma: "all",
    sortPackageJson: false,
    ignorePatterns: ["**/*.md", "**/generated/**", "lean/**", "audit/**", "crates/**"],
  },
  lint: {
    categories: { correctness: "error", suspicious: "warn" },
    ignorePatterns: ["node_modules", "dist", ".direnv", "**/generated/**", "target"],
    options: { typeAware: true, typeCheck: true },
  },
  test: {
    root: import.meta.dirname,
    include: ["apps/*/test/**/*.test.ts"],
  },
  plugins: [react(), tailwindcss()],
  server: { host: "127.0.0.1" },
  run: {
    tasks: {
      check: { command: "vp check && vp test", cache: { untrackedEnv: ["TSRS_BINARY"] } },
    },
  },
} satisfies ViteUserConfig)
