import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import CustomerUploadPortal from './CustomerUploadPortal'
import AdminPanel from './AdminPanel'
import './index.css'

function RootRouter() {
  const [currentPath, setCurrentPath] = React.useState(() => {
    return window.location.pathname + window.location.search + window.location.hash
  })

  React.useEffect(() => {
    const handleLocationChange = () => {
      setCurrentPath(window.location.pathname + window.location.search + window.location.hash)
    }
    window.addEventListener('popstate', handleLocationChange)
    window.addEventListener('hashchange', handleLocationChange)
    return () => {
      window.removeEventListener('popstate', handleLocationChange)
      window.removeEventListener('hashchange', handleLocationChange)
    }
  }, [])

  const lower = currentPath.toLowerCase()
  const isTauri = typeof window !== 'undefined' && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)

  // Super Admin route (/admin)
  if (lower.startsWith('/admin') || lower.includes('#/admin')) {
    return <AdminPanel />
  }

  // Shop Owner Web Panel (/shop-panel) or explicit dashboard mode
  if (lower.startsWith('/shop-panel') || lower.includes('#dashboard') || lower.includes('mode=dashboard')) {
    return <App />
  }

  // Check if previewing portal from Tauri desktop app
  if (lower.includes('preview-portal') || lower.includes('preview-upload') || lower.includes('#portal')) {
    return <CustomerUploadPortal />
  }

  // Check customer portal web routes: /portal, /upload, /shop, ?shop=
  const isCustomerRoute =
    lower.includes('/portal') ||
    lower.includes('/upload') ||
    lower.includes('/shop') ||
    lower.includes('shop=')

  // In non-Tauri web environment (Vercel / Mobile Browser), default to Customer Portal
  if (!isTauri) {
    return <CustomerUploadPortal />
  }

  // In Tauri desktop app with customer route
  if (isCustomerRoute) {
    return <CustomerUploadPortal />
  }

  // Default in Tauri Desktop App is the Shopkeeper Dashboard
  return <App />
}

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <RootRouter />
  </React.StrictMode>,
)
