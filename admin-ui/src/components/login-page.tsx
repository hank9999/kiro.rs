import { useState, useEffect } from 'react'
import { KeyRound, RefreshCw } from 'lucide-react'
import { storage } from '@/lib/storage'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { getCaptcha, login } from '@/api/auth'

interface LoginPageProps {
  onLogin: (token: string) => void
}

export function LoginPage({ onLogin }: LoginPageProps) {
  const [apiKey, setApiKey] = useState('')
  const [captchaToken, setCaptchaToken] = useState('')
  const [captchaImage, setCaptchaImage] = useState('')
  const [captchaAnswer, setCaptchaAnswer] = useState('')
  const [loadingCaptcha, setLoadingCaptcha] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState('')

  // 获取 CAPTCHA
  const fetchCaptcha = async () => {
    setLoadingCaptcha(true)
    setError('')
    setCaptchaAnswer('')

    try {
      const data = await getCaptcha()
      setCaptchaToken(data.token)
      setCaptchaImage(data.image)
    } catch (err) {
      setError('获取验证码失败，请重试')
      console.error('Failed to fetch CAPTCHA:', err)
    } finally {
      setLoadingCaptcha(false)
    }
  }

  // 组件挂载时获取 CAPTCHA
  useEffect(() => {
    fetchCaptcha()
  }, [])

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()

    if (!apiKey.trim() || !captchaAnswer.trim()) {
      setError('请填写完整信息')
      return
    }

    if (captchaAnswer.length !== 4) {
      setError('验证码必须是 4 位')
      return
    }

    setSubmitting(true)
    setError('')

    try {
      const response = await login({
        apiKey: apiKey.trim(),
        captchaToken,
        captchaAnswer: captchaAnswer.trim(),
      })

      // 登录成功，保存 token
      storage.setToken(response.token)
      onLogin(response.token)
    } catch (err: any) {
      console.error('Login failed:', err)

      // 提取错误信息
      if (err.response?.status === 429) {
        setError('登录尝试次数过多，请稍后再试')
      } else if (err.response?.data?.error?.message) {
        setError(err.response.data.error.message)
      } else {
        setError('登录失败，请检查密码和验证码')
      }

      // 刷新 CAPTCHA
      fetchCaptcha()
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-primary/10">
            <KeyRound className="h-6 w-6 text-primary" />
          </div>
          <CardTitle className="text-2xl">Kiro Admin</CardTitle>
          <CardDescription>
            请输入 Admin API Key 和验证码以访问管理面板
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
            {/* API Key 输入 */}
            <div className="space-y-2">
              <label htmlFor="apiKey" className="text-sm font-medium">
                Admin API Key
              </label>
              <Input
                id="apiKey"
                type="password"
                placeholder="输入 API Key"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                autoComplete="off"
                data-1p-ignore
              />
            </div>

            {/* CAPTCHA 显示 */}
            <div className="space-y-2">
              <label htmlFor="captcha" className="text-sm font-medium">
                验证码
              </label>
              <div className="flex items-center gap-2">
                <div className="flex-1 relative h-[60px] bg-muted rounded-md overflow-hidden border">
                  {loadingCaptcha ? (
                    <div className="absolute inset-0 flex items-center justify-center">
                      <RefreshCw className="h-5 w-5 animate-spin text-muted-foreground" />
                    </div>
                  ) : captchaImage ? (
                    <img
                      src={captchaImage}
                      alt="CAPTCHA"
                      className="w-full h-full object-contain"
                    />
                  ) : null}
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  onClick={fetchCaptcha}
                  disabled={loadingCaptcha}
                  title="刷新验证码"
                >
                  <RefreshCw className={`h-4 w-4 ${loadingCaptcha ? 'animate-spin' : ''}`} />
                </Button>
              </div>
            </div>

            {/* CAPTCHA 答案输入 */}
            <div className="space-y-2">
              <Input
                id="captcha"
                type="text"
                placeholder="输入验证码（4位，不区分大小写）"
                value={captchaAnswer}
                onChange={(e) => {
                  const value = e.target.value.replace(/[^a-zA-Z0-9]/g, '').slice(0, 4)
                  setCaptchaAnswer(value)
                }}
                maxLength={4}
                autoComplete="off"
                data-1p-ignore
              />
            </div>

            {/* 错误信息 */}
            {error && (
              <div className="text-sm text-destructive text-center p-2 bg-destructive/10 rounded-md">
                {error}
              </div>
            )}

            {/* 提交按钮 */}
            <Button
              type="submit"
              className="w-full"
              disabled={!apiKey.trim() || captchaAnswer.length !== 4 || submitting || loadingCaptcha}
            >
              {submitting ? '登录中...' : '登录'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
