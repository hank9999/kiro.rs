// Dashboard 页面
// Input: credentialsApi, authApi, importExportApi
// Output: 凭据管理界面
// Pos: 主管理界面

import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { toast } from 'react-toastify';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Checkbox } from '@/components/ui/checkbox';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { authApi, credentialsApi, importExportApi, type CredentialItem, type UserInfo, type ImportCredentialItem } from '@/api';
import { User, Plus, DotsThreeVertical, Gear, SignOut, Envelope, ArrowsClockwise, ArrowsCounterClockwise, Sun, Moon, Export, UploadSimple, Play } from '@phosphor-icons/react';
import { useTheme } from '@/hooks/useTheme';

export default function DashboardPage() {
  const navigate = useNavigate();
  const { theme, toggleTheme } = useTheme();
  const [user, setUser] = useState<UserInfo | null>(null);
  const [credentials, setCredentials] = useState<CredentialItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAddDialog, setShowAddDialog] = useState(false);
  const [showEditDialog, setShowEditDialog] = useState(false);
  const [showSettingsDialog, setShowSettingsDialog] = useState(false);
  const [editingCredential, setEditingCredential] = useState<CredentialItem | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [importJson, setImportJson] = useState('');
  const [addTab, setAddTab] = useState<'single' | 'batch'>('single');
  const [addForm, setAddForm] = useState({
    refreshToken: '',
    authMethod: 'social',
    clientId: '',
    clientSecret: '',
    priority: 0,
    region: 'us-east-1',
    proxyUrl: '',
  });
  const [editForm, setEditForm] = useState({
    priority: 0,
    region: 'us-east-1',
    machineId: '',
    refreshToken: '',
    clientId: '',
    clientSecret: '',
    proxyUrl: '',
  });
  const [passwordForm, setPasswordForm] = useState({
    oldPassword: '',
    newPassword: '',
    confirmPassword: '',
  });

  // 加载用户信息和凭据
  useEffect(() => {
    const loadData = async () => {
      try {
        const [userRes, credRes] = await Promise.all([
          authApi.me(),
          credentialsApi.getAll(),
        ]);
        setUser(userRes);
        setCredentials(credRes.credentials);
      } catch {
        toast.error('请先登录');
        navigate('/admin');
      } finally {
        setLoading(false);
      }
    };
    loadData();
  }, [navigate]);

  // 刷新凭据列表
  const refreshCredentials = async () => {
    try {
      const res = await credentialsApi.getAll();
      setCredentials(res.credentials);
    } catch (err) {
      toast.error('刷新失败');
    }
  };

  // 刷新所有凭据 Token
  const handleRefreshAll = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      const res = await credentialsApi.refreshAll();
      if (res.failures.length > 0) {
        toast.warning(`刷新完成: ${res.successCount} 成功, ${res.failures.length} 失败`);
      } else {
        toast.success(`刷新完成: ${res.successCount} 个凭据已更新`);
      }
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '刷新失败');
    } finally {
      setRefreshing(false);
    }
  };

  // 根据使用百分比获取进度条颜色
  const getProgressColor = (percentage: number) => {
    if (percentage < 30) return 'bg-green-500';
    if (percentage < 50) return 'bg-lime-500';
    if (percentage < 80) return 'bg-yellow-500';
    if (percentage < 100) return 'bg-orange-500';
    return 'bg-red-500';
  };

  // 格式化余额显示（当前使用/总额度）+ 进度条
  const formatBalance = (cred: CredentialItem) => {
    if (cred.usageLimit <= 0) {
      return <span className="text-muted-foreground">-</span>;
    }
    const percentage = Math.min((cred.currentUsage / cred.usageLimit) * 100, 100);
    const percentageText = percentage.toFixed(0);
    return (
      <div className="space-y-1 w-32">
        <span className="text-sm">
          {cred.currentUsage.toFixed(0)} / {cred.usageLimit.toFixed(0)}
          <span className="text-muted-foreground ml-1">({percentageText}%)</span>
        </span>
        <div className="w-full h-1.5 bg-muted rounded-full overflow-hidden">
          <div
            className={`h-full rounded-full transition-all duration-300 ${getProgressColor(percentage)}`}
            style={{ width: `${percentage}%` }}
          />
        </div>
      </div>
    );
  };

  // 生成随机 Machine ID（64位十六进制字符串）
  const generateMachineId = () => {
    const chars = '0123456789abcdef';
    let result = '';
    for (let i = 0; i < 64; i++) {
      result += chars[Math.floor(Math.random() * chars.length)];
    }
    return result;
  };

  // 登出
  const handleLogout = async () => {
    try {
      await authApi.logout();
      navigate('/admin');
    } catch {
      navigate('/admin');
    }
  };

  // 添加凭据
  const handleAddCredential = async () => {
    if (!addForm.refreshToken) {
      toast.error('请输入 Refresh Token');
      return;
    }
    try {
      await credentialsApi.add({
        refreshToken: addForm.refreshToken,
        authMethod: addForm.authMethod,
        clientId: addForm.clientId || undefined,
        clientSecret: addForm.clientSecret || undefined,
        priority: addForm.priority,
        region: addForm.region,
        proxyUrl: addForm.proxyUrl || undefined,
      });
      toast.success('添加成功');
      setShowAddDialog(false);
      setAddForm({ refreshToken: '', authMethod: 'social', clientId: '', clientSecret: '', priority: 0, region: 'us-east-1', proxyUrl: '' });
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '添加失败');
    }
  };

  // 切换禁用状态
  const handleToggleDisabled = async (id: number, disabled: boolean) => {
    try {
      await credentialsApi.setDisabled(id, !disabled);
      toast.success(disabled ? '已启用' : '已禁用');
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '操作失败');
    }
  };

  // 重置失败计数
  const handleReset = async (id: number) => {
    try {
      await credentialsApi.reset(id);
      toast.success('已重置');
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '重置失败');
    }
  };

  // 删除凭据
  const handleDelete = async (id: number) => {
    if (!confirm('确定要删除这个凭据吗？')) return;
    try {
      await credentialsApi.delete(id);
      toast.success('已删除');
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '删除失败');
    }
  };

  // 打开编辑对话框
  const openEditDialog = (cred: CredentialItem) => {
    setEditingCredential(cred);
    setEditForm({
      priority: cred.priority,
      region: cred.region || 'us-east-1',
      machineId: cred.machineId || '',
      refreshToken: '',
      clientId: '',
      clientSecret: '',
      proxyUrl: cred.proxyUrl || '',
    });
    setShowEditDialog(true);
  };

  // 更新凭据
  const handleUpdateCredential = async () => {
    if (!editingCredential) return;
    try {
      await credentialsApi.update(editingCredential.id, {
        priority: editForm.priority,
        region: editForm.region,
        machineId: editForm.machineId || undefined,
        refreshToken: editForm.refreshToken || undefined,
        clientId: editForm.clientId || undefined,
        clientSecret: editForm.clientSecret || undefined,
        proxyUrl: editForm.proxyUrl,
      });
      toast.success('更新成功');
      setShowEditDialog(false);
      setEditingCredential(null);
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '更新失败');
    }
  };

  // 修改密码
  const handleChangePassword = async () => {
    if (passwordForm.newPassword !== passwordForm.confirmPassword) {
      toast.error('两次输入的密码不一致');
      return;
    }
    if (passwordForm.newPassword.length < 6) {
      toast.error('新密码至少6位');
      return;
    }
    try {
      await authApi.changePassword({
        oldPassword: passwordForm.oldPassword,
        newPassword: passwordForm.newPassword,
      });
      toast.success('密码修改成功');
      setShowSettingsDialog(false);
      setPasswordForm({ oldPassword: '', newPassword: '', confirmPassword: '' });
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '修改失败');
    }
  };

  // 多选相关
  const toggleSelect = (id: number) => {
    const newSelected = new Set(selectedIds);
    if (newSelected.has(id)) {
      newSelected.delete(id);
    } else {
      newSelected.add(id);
    }
    setSelectedIds(newSelected);
  };

  const toggleSelectAll = () => {
    if (selectedIds.size === credentials.length) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(credentials.map(c => c.id)));
    }
  };

  // 使用此账号
  const handleUseCurrent = async (id: number) => {
    try {
      await importExportApi.useCurrent(id);
      toast.success(`已切换到凭据 #${id}`);
      refreshCredentials();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '切换失败');
    }
  };

  // 导出凭据
  const handleExport = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要导出的凭据');
      return;
    }

    try {
      const ids = Array.from(selectedIds);
      const res = await importExportApi.export({ ids });

      // 下载 JSON 文件
      const blob = new Blob([JSON.stringify(res.credentials, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `kiro-credentials-${new Date().toISOString().slice(0, 10)}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);

      toast.success(`成功导出 ${res.count} 个凭据`);
      setSelectedIds(new Set());
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '导出失败');
    }
  };

  // 导入凭据
  const handleImport = async () => {
    if (!importJson.trim()) {
      toast.error('请输入 JSON 数据');
      return;
    }

    try {
      const data = JSON.parse(importJson) as ImportCredentialItem | ImportCredentialItem[];
      const res = await importExportApi.import(data);

      if (res.failures.length > 0) {
        toast.warning(`导入完成: ${res.successCount} 成功, ${res.failures.length} 失败`);
      } else {
        toast.success(`成功导入 ${res.successCount} 个凭据`);
      }

      setShowAddDialog(false);
      setImportJson('');
      setAddTab('single');
      refreshCredentials();
    } catch (err) {
      if (err instanceof SyntaxError) {
        toast.error('JSON 格式错误');
      } else {
        toast.error(err instanceof Error ? err.message : '导入失败');
      }
    }
  };

  if (loading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="text-muted-foreground">加载中...</div>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gradient-to-b from-background to-accent/20">
      {/* Header */}
      <header className="border-b bg-card/80 backdrop-blur-sm sticky top-0 z-10">
        <div className="container mx-auto px-4 py-4 flex items-center justify-between">
          <h1 className="text-xl font-bold bg-gradient-to-r from-primary to-primary/70 bg-clip-text text-transparent">Kiro Admin</h1>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="icon" onClick={toggleTheme} className="hover:bg-accent">
              {theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
            </Button>
            <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="sm" className="gap-2">
                <User size={18} />
                {user?.username}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => navigate('/admin/settings')}>
                <Gear size={16} className="mr-2" />
                系统设置
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => setShowSettingsDialog(true)}>
                <Gear size={16} className="mr-2" />
                修改密码
              </DropdownMenuItem>
              <DropdownMenuItem onClick={handleLogout}>
                <SignOut size={16} className="mr-2" />
                退出登录
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="container mx-auto px-4 py-6">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between">
            <CardTitle>凭据管理</CardTitle>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={handleRefreshAll}
                disabled={refreshing}
              >
                <ArrowsClockwise size={16} className={`mr-1 ${refreshing ? 'animate-spin' : ''}`} />
                {refreshing ? '刷新中...' : '刷新 Token'}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={handleExport}
                disabled={selectedIds.size === 0}
              >
                <Export size={16} className="mr-1" />
                导出 Token {selectedIds.size > 0 && `(${selectedIds.size})`}
              </Button>
              <Button size="sm" onClick={() => setShowAddDialog(true)}>
                <Plus size={16} className="mr-1" />
                添加凭据
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-10">
                    <Checkbox
                      checked={credentials.length > 0 && selectedIds.size === credentials.length}
                      onCheckedChange={toggleSelectAll}
                    />
                  </TableHead>
                  <TableHead>ID</TableHead>
                  <TableHead>Email</TableHead>
                  <TableHead>订阅计划</TableHead>
                  <TableHead>使用额度</TableHead>
                  <TableHead>状态</TableHead>
                  <TableHead>Token有效期</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {credentials.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={8} className="text-center text-muted-foreground py-8">
                      暂无凭据，点击"添加凭据"开始
                    </TableCell>
                  </TableRow>
                ) : (
                  credentials.map((cred) => (
                    <TableRow key={cred.id}>
                      <TableCell>
                        <Checkbox
                          checked={selectedIds.has(cred.id)}
                          onCheckedChange={() => toggleSelect(cred.id)}
                        />
                      </TableCell>
                      <TableCell>
                        #{cred.id}
                        {cred.isCurrent && (
                          <Badge variant="default" className="ml-2">当前</Badge>
                        )}
                      </TableCell>
                      <TableCell>
                        {cred.email ? (
                          <span className="flex items-center gap-1 text-sm">
                            <Envelope size={14} className="text-muted-foreground" />
                            {cred.email}
                          </span>
                        ) : (
                          <span className="text-muted-foreground">-</span>
                        )}
                      </TableCell>
                      <TableCell>
                        {cred.subscriptionTitle || <span className="text-muted-foreground">-</span>}
                      </TableCell>
                      <TableCell>
                        {formatBalance(cred)}
                      </TableCell>
                      <TableCell>
                        {cred.disabled ? (
                          cred.disabledReason === 'suspended' ? (
                            <Badge variant="destructive">暂停</Badge>
                          ) : cred.disabledReason === 'quota' ? (
                            <Badge variant="destructive">额度用尽</Badge>
                          ) : (
                            <Badge variant="secondary">禁用</Badge>
                          )
                        ) : cred.failureCount > 0 ? (
                          <Badge variant="secondary">失败 {cred.failureCount}</Badge>
                        ) : (
                          <Badge variant="default">正常</Badge>
                        )}
                      </TableCell>
                      <TableCell>
                        {cred.expiresAt
                          ? new Date(cred.expiresAt).toLocaleString()
                          : '-'}
                      </TableCell>
                      <TableCell className="text-right">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon">
                              <DotsThreeVertical size={16} />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            {!cred.isCurrent && !cred.disabled && cred.failureCount === 0 && (
                              <DropdownMenuItem onClick={() => handleUseCurrent(cred.id)}>
                                <Play size={16} className="mr-2" />
                                使用此账号
                              </DropdownMenuItem>
                            )}
                            <DropdownMenuItem onClick={() => openEditDialog(cred)}>
                              编辑
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onClick={() => handleToggleDisabled(cred.id, cred.disabled)}
                            >
                              {cred.disabled ? '启用' : '禁用'}
                            </DropdownMenuItem>
                            <DropdownMenuItem onClick={() => handleReset(cred.id)}>
                              重置失败计数
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              className="text-destructive"
                              onClick={() => handleDelete(cred.id)}
                            >
                              删除
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      </main>

      {/* 添加凭据对话框 */}
      <Dialog open={showAddDialog} onOpenChange={(open) => {
        setShowAddDialog(open);
        if (!open) {
          setAddTab('single');
          setImportJson('');
        }
      }}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>添加凭据</DialogTitle>
            <DialogDescription>
              添加单个凭据或批量导入
            </DialogDescription>
          </DialogHeader>
          <Tabs value={addTab} onValueChange={(v) => setAddTab(v as 'single' | 'batch')}>
            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="single">单个添加</TabsTrigger>
              <TabsTrigger value="batch">批量导入</TabsTrigger>
            </TabsList>
            <TabsContent value="single" className="space-y-4 py-4">
              <div className="space-y-2">
                <Label>Refresh Token *</Label>
                <Input
                  placeholder="请输入 Refresh Token"
                  value={addForm.refreshToken}
                  onChange={(e) => setAddForm({ ...addForm, refreshToken: e.target.value })}
                />
              </div>
              <div className="space-y-2">
                <Label>认证方式</Label>
                <select
                  className="w-full h-10 px-3 rounded-md border bg-background"
                  value={addForm.authMethod}
                  onChange={(e) => setAddForm({ ...addForm, authMethod: e.target.value })}
                >
                  <option value="social">Social</option>
                  <option value="idc">IdC</option>
                </select>
              </div>
              {addForm.authMethod === 'idc' && (
                <>
                  <div className="space-y-2">
                    <Label>Client ID</Label>
                    <Input
                      placeholder="请输入 Client ID"
                      value={addForm.clientId}
                      onChange={(e) => setAddForm({ ...addForm, clientId: e.target.value })}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>Client Secret</Label>
                    <Input
                      type="password"
                      placeholder="请输入 Client Secret"
                      value={addForm.clientSecret}
                      onChange={(e) => setAddForm({ ...addForm, clientSecret: e.target.value })}
                    />
                  </div>
                </>
              )}
              <div className="space-y-2">
                <Label>优先级</Label>
                <Input
                  type="number"
                  placeholder="0"
                  value={addForm.priority}
                  onChange={(e) => setAddForm({ ...addForm, priority: parseInt(e.target.value) || 0 })}
                />
                <p className="text-xs text-muted-foreground">数字越小优先级越高</p>
              </div>
              <div className="space-y-2">
                <Label>AWS Region</Label>
                <select
                  className="w-full h-10 px-3 rounded-md border bg-background"
                  value={addForm.region}
                  onChange={(e) => setAddForm({ ...addForm, region: e.target.value })}
                >
                  <option value="us-east-1">us-east-1 (N. Virginia)</option>
                  <option value="us-west-2">us-west-2 (Oregon)</option>
                  <option value="eu-west-1">eu-west-1 (Ireland)</option>
                </select>
              </div>
              <div className="space-y-2">
                <Label>代理 URL</Label>
                <Input
                  placeholder="http://proxy:port 或 socks5://proxy:port（可选）"
                  value={addForm.proxyUrl}
                  onChange={(e) => setAddForm({ ...addForm, proxyUrl: e.target.value })}
                />
                <p className="text-xs text-muted-foreground">支持 http/https/socks5 代理，留空则不使用代理</p>
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => setShowAddDialog(false)}>
                  取消
                </Button>
                <Button onClick={handleAddCredential}>添加</Button>
              </DialogFooter>
            </TabsContent>
            <TabsContent value="batch" className="space-y-4 py-4">
              <div className="space-y-2">
                <Label>JSON 数据</Label>
                <textarea
                  className="w-full h-64 px-3 py-2 rounded-md border bg-background font-mono text-sm resize-none"
                  placeholder={`支持以下格式：

单个凭据：
{
  "refreshToken": "your_refresh_token",
  "clientId": "",
  "clientSecret": ""
}

多个凭据：
[
  { "refreshToken": "token1" },
  { "refreshToken": "token2", "clientId": "...", "clientSecret": "..." }
]

说明：
- refreshToken: 必填
- clientId + clientSecret: 都有值则为 IdC 模式，否则为 Social 模式
- region: 可选，默认 us-east-1
- proxyUrl: 可选，默认为空`}
                  value={importJson}
                  onChange={(e) => setImportJson(e.target.value)}
                />
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => setShowAddDialog(false)}>
                  取消
                </Button>
                <Button onClick={handleImport}>
                  <UploadSimple size={16} className="mr-1" />
                  导入
                </Button>
              </DialogFooter>
            </TabsContent>
          </Tabs>
        </DialogContent>
      </Dialog>

      {/* 编辑凭据对话框 */}
      <Dialog open={showEditDialog} onOpenChange={setShowEditDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>编辑凭据 #{editingCredential?.id}</DialogTitle>
            <DialogDescription>
              修改凭据配置（认证方式: {editingCredential?.authMethod || 'social'}）
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4">
            {/* 根据认证方式显示不同的 Token 字段 */}
            {editingCredential?.authMethod === 'idc' ? (
              <>
                <div className="space-y-2">
                  <Label>Client ID</Label>
                  <Input
                    placeholder="留空则不修改"
                    value={editForm.clientId}
                    onChange={(e) => setEditForm({ ...editForm, clientId: e.target.value })}
                  />
                </div>
                <div className="space-y-2">
                  <Label>Client Secret</Label>
                  <Input
                    type="password"
                    placeholder="留空则不修改"
                    value={editForm.clientSecret}
                    onChange={(e) => setEditForm({ ...editForm, clientSecret: e.target.value })}
                  />
                </div>
                <div className="space-y-2">
                  <Label>Refresh Token</Label>
                  <Input
                    type="password"
                    placeholder="留空则不修改"
                    value={editForm.refreshToken}
                    onChange={(e) => setEditForm({ ...editForm, refreshToken: e.target.value })}
                  />
                </div>
              </>
            ) : (
              <div className="space-y-2">
                <Label>Refresh Token</Label>
                <Input
                  type="password"
                  placeholder="留空则不修改"
                  value={editForm.refreshToken}
                  onChange={(e) => setEditForm({ ...editForm, refreshToken: e.target.value })}
                />
              </div>
            )}
            <div className="space-y-2">
              <Label>优先级</Label>
              <Input
                type="number"
                placeholder="0"
                value={editForm.priority}
                onChange={(e) => setEditForm({ ...editForm, priority: parseInt(e.target.value) || 0 })}
              />
              <p className="text-xs text-muted-foreground">数字越小优先级越高</p>
            </div>
            <div className="space-y-2">
              <Label>AWS Region</Label>
              <select
                className="w-full h-10 px-3 rounded-md border bg-background"
                value={editForm.region}
                onChange={(e) => setEditForm({ ...editForm, region: e.target.value })}
              >
                <option value="us-east-1">us-east-1 (N. Virginia)</option>
                <option value="us-west-2">us-west-2 (Oregon)</option>
                <option value="eu-west-1">eu-west-1 (Ireland)</option>
              </select>
            </div>
            <div className="space-y-2">
              <Label>Machine ID</Label>
              <div className="flex gap-2">
                <Input
                  placeholder="64位十六进制字符串（留空自动生成）"
                  value={editForm.machineId}
                  onChange={(e) => setEditForm({ ...editForm, machineId: e.target.value })}
                  className="flex-1"
                />
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  onClick={() => setEditForm({ ...editForm, machineId: generateMachineId() })}
                  title="生成随机 Machine ID"
                >
                  <ArrowsCounterClockwise size={16} />
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">留空则使用自动生成的值</p>
            </div>
            <div className="space-y-2">
              <Label>代理 URL</Label>
              <Input
                placeholder="http://proxy:port 或 socks5://proxy:port（可选）"
                value={editForm.proxyUrl}
                onChange={(e) => setEditForm({ ...editForm, proxyUrl: e.target.value })}
              />
              <p className="text-xs text-muted-foreground">支持 http/https/socks5 代理，留空则不使用代理</p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowEditDialog(false)}>
              取消
            </Button>
            <Button onClick={handleUpdateCredential}>保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 修改密码对话框 */}
      <Dialog open={showSettingsDialog} onOpenChange={setShowSettingsDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>修改密码</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <Label>原密码</Label>
              <Input
                type="password"
                placeholder="请输入原密码"
                value={passwordForm.oldPassword}
                onChange={(e) => setPasswordForm({ ...passwordForm, oldPassword: e.target.value })}
              />
            </div>
            <div className="space-y-2">
              <Label>新密码</Label>
              <Input
                type="password"
                placeholder="请输入新密码"
                value={passwordForm.newPassword}
                onChange={(e) => setPasswordForm({ ...passwordForm, newPassword: e.target.value })}
              />
            </div>
            <div className="space-y-2">
              <Label>确认新密码</Label>
              <Input
                type="password"
                placeholder="请再次输入新密码"
                value={passwordForm.confirmPassword}
                onChange={(e) => setPasswordForm({ ...passwordForm, confirmPassword: e.target.value })}
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowSettingsDialog(false)}>
              取消
            </Button>
            <Button onClick={handleChangePassword}>确认修改</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
