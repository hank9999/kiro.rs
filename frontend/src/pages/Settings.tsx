// Input: @/api (settingsApi, authApi), @/components/ui (UI组件), react-router-dom, react-toastify
// Output: 系统设置页面组件
// Pos: 独立的系统设置管理页面

import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { toast } from 'react-toastify';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { settingsApi, authApi } from '@/api';
import { ArrowLeft, Sun, Moon } from '@phosphor-icons/react';
import { useTheme } from '@/hooks/useTheme';

export default function SettingsPage() {
  const navigate = useNavigate();
  const { theme, toggleTheme } = useTheme();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [form, setForm] = useState({
    minUsageThreshold: 5,
    proxyUrl: '',
    countTokensApiUrl: '',
    countTokensApiKey: '',
    countTokensAuthType: 'x-api-key',
    kiroVersion: '',
    systemVersion: '',
    nodeVersion: '',
  });

  // 加载设置
  useEffect(() => {
    const loadSettings = async () => {
      try {
        // 验证登录状态
        await authApi.me();
        const res = await settingsApi.get();
        setForm({
          minUsageThreshold: res.minUsageThreshold,
          proxyUrl: res.proxyUrl || '',
          countTokensApiUrl: res.countTokensApiUrl || '',
          countTokensApiKey: res.countTokensApiKey || '',
          countTokensAuthType: res.countTokensAuthType,
          kiroVersion: res.kiroVersion,
          systemVersion: res.systemVersion,
          nodeVersion: res.nodeVersion,
        });
      } catch {
        navigate('/admin');
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
  }, [navigate]);

  // 保存设置
  const handleSave = async () => {
    setSaving(true);
    try {
      await settingsApi.update(form);
      toast.success('设置已保存');
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '保存失败');
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <p className="text-muted-foreground">加载中...</p>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gradient-to-b from-background to-accent/20">
      {/* Header */}
      <header className="border-b bg-card/80 backdrop-blur-sm sticky top-0 z-10">
        <div className="container mx-auto px-4 py-4 flex items-center justify-between">
          <div className="flex items-center gap-4">
            <Button variant="ghost" size="icon" onClick={() => navigate('/admin/dashboard')} className="hover:bg-accent">
              <ArrowLeft size={20} />
            </Button>
            <h1 className="text-xl font-semibold">系统设置</h1>
          </div>
          <Button variant="ghost" size="icon" onClick={toggleTheme} className="hover:bg-accent">
            {theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
          </Button>
        </div>
      </header>

      {/* Main Content */}
      <main className="container mx-auto px-4 py-6 max-w-3xl space-y-6">
        {/* 额度管理 */}
        <Card>
          <CardHeader>
            <CardTitle>额度管理</CardTitle>
            <CardDescription>配置凭据额度相关参数</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label>最低额度阈值</Label>
              <Input
                type="number"
                placeholder="5"
                value={form.minUsageThreshold}
                onChange={(e) => setForm({ ...form, minUsageThreshold: parseFloat(e.target.value) || 0 })}
              />
              <p className="text-xs text-muted-foreground">
                当剩余额度低于此值时自动切换到下一个账号
              </p>
            </div>
          </CardContent>
        </Card>

        {/* 代理设置 */}
        <Card>
          <CardHeader>
            <CardTitle>代理设置</CardTitle>
            <CardDescription>配置网络代理</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label>Proxy URL</Label>
              <Input
                placeholder="http://127.0.0.1:7890"
                value={form.proxyUrl}
                onChange={(e) => setForm({ ...form, proxyUrl: e.target.value })}
              />
              <p className="text-xs text-muted-foreground">
                HTTP/HTTPS 代理地址，留空则不使用代理
              </p>
            </div>
          </CardContent>
        </Card>

        {/* Count Tokens API */}
        <Card>
          <CardHeader>
            <CardTitle>Count Tokens API</CardTitle>
            <CardDescription>配置外部 Token 计数 API（可选）</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label>API URL</Label>
              <Input
                placeholder="https://api.example.com/count_tokens"
                value={form.countTokensApiUrl}
                onChange={(e) => setForm({ ...form, countTokensApiUrl: e.target.value })}
              />
              <p className="text-xs text-muted-foreground">
                外部 count_tokens API 地址，留空则不使用
              </p>
            </div>
            <div className="space-y-2">
              <Label>API Key</Label>
              <Input
                type="password"
                placeholder="sk-..."
                value={form.countTokensApiKey}
                onChange={(e) => setForm({ ...form, countTokensApiKey: e.target.value })}
              />
              <p className="text-xs text-muted-foreground">
                外部 API 密钥
              </p>
            </div>
            <div className="space-y-2">
              <Label>认证类型</Label>
              <select
                className="w-full h-10 px-3 rounded-md border bg-background"
                value={form.countTokensAuthType}
                onChange={(e) => setForm({ ...form, countTokensAuthType: e.target.value })}
              >
                <option value="x-api-key">X-API-Key</option>
                <option value="bearer">Bearer Token</option>
              </select>
              <p className="text-xs text-muted-foreground">
                API 认证方式
              </p>
            </div>
          </CardContent>
        </Card>

        {/* Kiro 版本信息 */}
        <Card>
          <CardHeader>
            <CardTitle>版本信息</CardTitle>
            <CardDescription>配置 Kiro 客户端模拟参数</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label>Kiro Version</Label>
              <Input
                placeholder="0.8.0"
                value={form.kiroVersion}
                onChange={(e) => setForm({ ...form, kiroVersion: e.target.value })}
              />
            </div>
            <div className="space-y-2">
              <Label>System Version</Label>
              <Input
                placeholder="darwin#24.6.0"
                value={form.systemVersion}
                onChange={(e) => setForm({ ...form, systemVersion: e.target.value })}
              />
            </div>
            <div className="space-y-2">
              <Label>Node Version</Label>
              <Input
                placeholder="v22.12.0"
                value={form.nodeVersion}
                onChange={(e) => setForm({ ...form, nodeVersion: e.target.value })}
              />
            </div>
          </CardContent>
        </Card>

        {/* 保存按钮 */}
        <div className="flex justify-end">
          <Button onClick={handleSave} disabled={saving}>
            {saving ? '保存中...' : '保存设置'}
          </Button>
        </div>
      </main>
    </div>
  );
}
