#!/usr/bin/env node

export const usage = "Usage: sanchoris-agent <command>\n"

export function main(): number {
  process.stdout.write(usage)
  return 0
}

if (import.meta.main) process.exitCode = main()
