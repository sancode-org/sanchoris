import { ApolloProvider } from "@apollo/client/react"
import { RouterProvider } from "@tanstack/react-router"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { apolloClient } from "./apollo.ts"
import { router } from "./router.tsx"
import "./styles.css"

const root = document.getElementById("root")
if (!root) throw new Error("the #root element is missing")

createRoot(root).render(
  <StrictMode>
    <ApolloProvider client={apolloClient}>
      <RouterProvider router={router} />
    </ApolloProvider>
  </StrictMode>,
)
