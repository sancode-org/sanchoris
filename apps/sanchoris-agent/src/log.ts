export type Logger = (
  level: "debug" | "info" | "warn" | "error",
  message: string,
  fields?: Record<string, unknown>,
) => void

export const stderrLogger: Logger = (level, message, fields = {}) => {
  process.stderr.write(`${JSON.stringify({ level, message, ...fields })}\n`)
}
