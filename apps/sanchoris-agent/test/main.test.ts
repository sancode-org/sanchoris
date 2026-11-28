import { expect, test, vi } from "vite-plus/test"
import { main, usage } from "../src/main.ts"

test("main prints the usage and succeeds", () => {
  using write = vi.spyOn(process.stdout, "write").mockImplementation(() => true)
  expect(main()).toBe(0)
  expect(write).toHaveBeenCalledWith(usage)
})
