import { createRootRoute, createRoute, createRouter, Outlet } from "@tanstack/react-router"
import { Workflow } from "lucide-react"
import { Link } from "react-aria-components"

const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-screen bg-white text-neutral-900">
      <header className="flex items-center gap-2 border-b border-neutral-200 px-4 py-3">
        <Workflow aria-hidden className="size-5" />
        <Link href="/" className="font-semibold outline-none data-focus-visible:ring-2">
          sanchoris
        </Link>
      </header>
      <Outlet />
    </div>
  ),
})

const homeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: () => <main className="p-4" />,
})

export const router = createRouter({ routeTree: rootRoute.addChildren([homeRoute]) })

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router
  }
}
