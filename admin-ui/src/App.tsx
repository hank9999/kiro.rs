import { useState, useEffect } from 'react'
import { storage } from '@/lib/storage'
import { LoginPage } from '@/components/login-page'
import { Dashboard } from '@/components/dashboard'
import { SettingsPage } from '@/components/settings-page'
import { Toaster } from '@/components/ui/sonner'

function App() {
  const [isLoggedIn, setIsLoggedIn] = useState(false)
  const [page, setPage] = useState<'dashboard' | 'settings'>('dashboard')

  useEffect(() => {
    // 检查是否已经有保存的 API Key
    if (storage.getApiKey()) {
      setIsLoggedIn(true)
    }
  }, [])

  const handleLogin = () => {
    setIsLoggedIn(true)
  }

  const handleLogout = () => {
    setPage('dashboard')
    setIsLoggedIn(false)
  }

  return (
    <>
      {isLoggedIn ? (
        page === 'settings' ? (
          <SettingsPage
            onBack={() => setPage('dashboard')}
            onLogout={handleLogout}
          />
        ) : (
          <Dashboard
            onLogout={handleLogout}
            onOpenSettings={() => setPage('settings')}
          />
        )
      ) : (
        <LoginPage onLogin={handleLogin} />
      )}
      <Toaster position="top-right" />
    </>
  )
}

export default App
