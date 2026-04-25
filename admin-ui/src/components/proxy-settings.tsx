import { useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  Globe,
  Loader2,
  PlayCircle,
  RefreshCw,
  Save,
  Snowflake,
  XCircle,
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import {
  useProxyPool,
  useTestProxyPool,
  useUpdateProxyPool,
} from "@/hooks/use-proxy";
import type {
  ProxyPoolConfig,
  ProxyPoolTemplate,
  ProxyStrategy,
  ProxyTestItem,
} from "@/types/api";
import { toast } from "sonner";

const STRATEGY_LABELS: Record<ProxyStrategy, string> = {
  "round-robin": "轮询（Round-Robin）",
  random: "随机",
  "per-credential": "按凭据绑定",
};

const DEFAULT_TEMPLATE: ProxyPoolTemplate = {
  protocol: "socks5h",
  host: "",
  portStart: 10001,
  portEnd: 10010,
};

interface FormState {
  enabled: boolean;
  strategy: ProxyStrategy;
  useTemplate: boolean;
  urlsText: string;
  template: ProxyPoolTemplate;
  username: string;
  password: string;
  testUrl: string;
  cooldownSecs: string;
}

function configToForm(cfg?: ProxyPoolConfig): FormState {
  if (!cfg) {
    return {
      enabled: false,
      strategy: "round-robin",
      useTemplate: true,
      urlsText: "",
      template: { ...DEFAULT_TEMPLATE },
      username: "",
      password: "",
      testUrl: "",
      cooldownSecs: "",
    };
  }
  return {
    enabled: cfg.enabled,
    strategy: cfg.strategy ?? "round-robin",
    useTemplate: !!cfg.template && (!cfg.urls || cfg.urls.length === 0),
    urlsText: cfg.urls?.join("\n") ?? "",
    template: cfg.template
      ? { ...cfg.template }
      : { ...DEFAULT_TEMPLATE },
    username: cfg.username ?? "",
    password: cfg.password ?? "",
    testUrl: cfg.testUrl ?? "",
    cooldownSecs:
      typeof cfg.cooldownSecs === "number" ? String(cfg.cooldownSecs) : "",
  };
}

function formToConfig(form: FormState): ProxyPoolConfig {
  const urls = form.urlsText
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  const payload: ProxyPoolConfig = {
    enabled: form.enabled,
    strategy: form.strategy,
  };

  if (form.useTemplate) {
    payload.template = { ...form.template };
  } else if (urls.length > 0) {
    payload.urls = urls;
  }

  if (form.username.trim()) payload.username = form.username.trim();

  // "***" 代表保留原密码（后端识别），空串代表清空，其他值代表更新
  if (form.password === "***") {
    payload.password = "***";
  } else if (form.password.length > 0) {
    payload.password = form.password;
  }

  if (form.testUrl.trim()) payload.testUrl = form.testUrl.trim();

  const cd = Number(form.cooldownSecs);
  if (form.cooldownSecs.trim() !== "" && Number.isFinite(cd) && cd > 0) {
    payload.cooldownSecs = Math.floor(cd);
  }

  return payload;
}

export function ProxySettings() {
  const { data, isLoading, error, refetch, isFetching } = useProxyPool();
  const updateMutation = useUpdateProxyPool();
  const testMutation = useTestProxyPool();

  const [form, setForm] = useState<FormState>(() => configToForm(data?.config));
  const [testResults, setTestResults] = useState<ProxyTestItem[] | null>(null);
  const [testSummary, setTestSummary] = useState<{
    total: number;
    success: number;
    failed: number;
    testUrl: string;
  } | null>(null);

  useEffect(() => {
    if (data?.config) {
      setForm(configToForm(data.config));
    }
  }, [data?.config?.enabled, data?.config?.strategy]); // eslint-disable-line react-hooks/exhaustive-deps

  // 每秒触发一次重渲染，让冷却倒计时在前端本地连续更新
  // （不依赖后端轮询，否则只能每 5 秒跳一次）
  const [, setTick] = useState(0);
  useEffect(() => {
    const now = Date.now();
    const hasActiveCooldown = data?.proxies?.some(
      (p) => p.cooldownUntilMs > now,
    );
    if (!hasActiveCooldown) return;
    const id = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, [data?.proxies]);

  const proxies = data?.proxies ?? [];
  // 前后端通常对齐 NTP，秒级倒计时不做时钟校正（若做校正反而会因 clockSkewMs
  // 随每次渲染重新计算而相互抵消，导致倒计时卡死）
  const activeCooldownCount = proxies.reduce(
    (acc, p) => acc + (p.cooldownUntilMs > Date.now() ? 1 : 0),
    0,
  );

  const validationError = useMemo(() => {
    if (!form.enabled) return null;
    if (form.useTemplate) {
      if (!form.template.host.trim()) return "请填写模板 host";
      if (form.template.portEnd < form.template.portStart)
        return "portEnd 必须 ≥ portStart";
    } else {
      const urls = form.urlsText
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean);
      if (urls.length === 0) return "URL 列表为空";
    }
    return null;
  }, [form]);

  const handleSave = async () => {
    if (validationError) {
      toast.error(validationError);
      return;
    }
    try {
      const payload = formToConfig(form);
      await updateMutation.mutateAsync(payload);
      toast.success("代理池配置已保存并热更新");
      refetch();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(`保存失败: ${msg}`);
    }
  };

  const handleTest = async () => {
    try {
      const resp = await testMutation.mutateAsync({
        testUrl: form.testUrl.trim() || undefined,
        timeoutSecs: 10,
      });
      setTestResults(resp.results);
      setTestSummary({
        total: resp.total,
        success: resp.success,
        failed: resp.failed,
        testUrl: resp.testUrl,
      });
      toast.success(
        `测试完成：${resp.success}/${resp.total} 成功，失败 ${resp.failed}`,
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(`测试失败: ${msg}`);
    }
  };

  if (error) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-center space-y-2">
          <p className="text-destructive">加载代理池配置失败</p>
          <Button variant="outline" onClick={() => refetch()}>
            重试
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* 统计卡片 */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">代理池状态</CardTitle>
            <Globe className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              {data?.active ? (
                <Badge className="bg-green-600 hover:bg-green-700">已启用</Badge>
              ) : (
                <Badge variant="secondary">未启用</Badge>
              )}
              <span className="text-sm text-muted-foreground">
                {data?.active ? "正在工作" : "全部请求直连或走单代理"}
              </span>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">池大小</CardTitle>
            <Globe className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{data?.size ?? 0}</div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">策略</CardTitle>
            <Globe className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-lg font-semibold">
              {STRATEGY_LABELS[data?.config.strategy ?? "round-robin"]}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* 操作栏 */}
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold">代理池设置</h2>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => refetch()}
            disabled={isFetching}
          >
            <RefreshCw
              className={`h-4 w-4 mr-1 ${isFetching ? "animate-spin" : ""}`}
            />
            刷新
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleTest}
            disabled={
              testMutation.isPending || !data?.active || (data?.size ?? 0) === 0
            }
          >
            {testMutation.isPending ? (
              <Loader2 className="h-4 w-4 mr-1 animate-spin" />
            ) : (
              <PlayCircle className="h-4 w-4 mr-1" />
            )}
            测试连通性
          </Button>
          <Button
            size="sm"
            onClick={handleSave}
            disabled={updateMutation.isPending || !!validationError}
          >
            {updateMutation.isPending ? (
              <Loader2 className="h-4 w-4 mr-1 animate-spin" />
            ) : (
              <Save className="h-4 w-4 mr-1" />
            )}
            保存并应用
          </Button>
        </div>
      </div>

      {/* 配置区 */}
      <Card>
        <CardHeader>
          <CardTitle>基础配置</CardTitle>
        </CardHeader>
        <CardContent className="space-y-6">
          {isLoading ? (
            <div className="text-muted-foreground">加载中...</div>
          ) : (
            <>
              {/* 启用 + 策略 */}
              <div className="grid gap-4 md:grid-cols-2">
                <div className="flex items-center justify-between space-x-2 rounded-md border p-4">
                  <div className="space-y-0.5">
                    <Label className="text-base">启用代理池</Label>
                    <p className="text-xs text-muted-foreground">
                      关闭后将回退到全局单代理或直连
                    </p>
                  </div>
                  <Switch
                    checked={form.enabled}
                    onCheckedChange={(v) =>
                      setForm((f) => ({ ...f, enabled: v }))
                    }
                  />
                </div>

                <div className="rounded-md border p-4 space-y-2">
                  <Label>选择策略</Label>
                  <select
                    className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    value={form.strategy}
                    onChange={(e) =>
                      setForm((f) => ({
                        ...f,
                        strategy: e.target.value as ProxyStrategy,
                      }))
                    }
                  >
                    <option value="round-robin">
                      {STRATEGY_LABELS["round-robin"]}
                    </option>
                    <option value="random">{STRATEGY_LABELS["random"]}</option>
                    <option value="per-credential">
                      {STRATEGY_LABELS["per-credential"]}
                    </option>
                  </select>
                  <p className="text-xs text-muted-foreground">
                    {form.strategy === "round-robin"
                      ? "每次请求按顺序轮换代理，推荐避免同一 IP 被限流"
                      : form.strategy === "random"
                        ? "每次请求从池中随机选择一个代理"
                        : "同一凭据始终使用相同代理（凭据ID 对池长度取模）"}
                  </p>
                </div>
              </div>

              {/* 来源切换 */}
              <div className="rounded-md border p-4 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="space-y-0.5">
                    <Label className="text-base">代理来源</Label>
                    <p className="text-xs text-muted-foreground">
                      选择使用端口范围模板或手动 URL 列表
                    </p>
                  </div>
                  <div className="flex items-center gap-2 text-sm">
                    <span
                      className={
                        form.useTemplate
                          ? "text-foreground font-medium"
                          : "text-muted-foreground"
                      }
                    >
                      模板
                    </span>
                    <Switch
                      checked={!form.useTemplate}
                      onCheckedChange={(v) =>
                        setForm((f) => ({ ...f, useTemplate: !v }))
                      }
                    />
                    <span
                      className={
                        !form.useTemplate
                          ? "text-foreground font-medium"
                          : "text-muted-foreground"
                      }
                    >
                      URL 列表
                    </span>
                  </div>
                </div>

                {form.useTemplate ? (
                  <div className="grid gap-3 md:grid-cols-4 mt-3">
                    <div className="space-y-1">
                      <Label>协议</Label>
                      <select
                        className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        value={form.template.protocol}
                        onChange={(e) =>
                          setForm((f) => ({
                            ...f,
                            template: {
                              ...f.template,
                              protocol: e.target.value,
                            },
                          }))
                        }
                      >
                        <option value="socks5h">socks5h</option>
                        <option value="socks5">socks5</option>
                        <option value="http">http</option>
                        <option value="https">https</option>
                      </select>
                    </div>
                    <div className="space-y-1 md:col-span-2">
                      <Label>主机</Label>
                      <Input
                        placeholder="dc.decodo.com"
                        value={form.template.host}
                        onChange={(e) =>
                          setForm((f) => ({
                            ...f,
                            template: { ...f.template, host: e.target.value },
                          }))
                        }
                      />
                    </div>
                    <div className="space-y-1">
                      <Label>起始端口</Label>
                      <Input
                        type="number"
                        value={form.template.portStart}
                        onChange={(e) =>
                          setForm((f) => ({
                            ...f,
                            template: {
                              ...f.template,
                              portStart: Number(e.target.value) || 0,
                            },
                          }))
                        }
                      />
                    </div>
                    <div className="space-y-1">
                      <Label>结束端口</Label>
                      <Input
                        type="number"
                        value={form.template.portEnd}
                        onChange={(e) =>
                          setForm((f) => ({
                            ...f,
                            template: {
                              ...f.template,
                              portEnd: Number(e.target.value) || 0,
                            },
                          }))
                        }
                      />
                    </div>
                    <p className="md:col-span-4 text-xs text-muted-foreground">
                      将展开为 {form.template.portStart} ~ {form.template.portEnd} 的 {Math.max(0, form.template.portEnd - form.template.portStart + 1)} 个代理
                    </p>
                  </div>
                ) : (
                  <div className="space-y-1 mt-3">
                    <Label>代理 URL 列表（每行一个）</Label>
                    <textarea
                      rows={6}
                      className="flex w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                      placeholder={"socks5h://user:pass@host:10001\nsocks5h://user:pass@host:10002"}
                      value={form.urlsText}
                      onChange={(e) =>
                        setForm((f) => ({ ...f, urlsText: e.target.value }))
                      }
                    />
                    <p className="text-xs text-muted-foreground">
                      URL 中若已带认证信息，下方全局认证将被忽略
                    </p>
                  </div>
                )}
              </div>

              {/* 全局认证 + 测试 URL + 冷却时长 */}
              <div className="grid gap-3 md:grid-cols-4">
                <div className="space-y-1">
                  <Label>全局用户名（可选）</Label>
                  <Input
                    placeholder="代理认证用户名"
                    value={form.username}
                    onChange={(e) =>
                      setForm((f) => ({ ...f, username: e.target.value }))
                    }
                  />
                </div>
                <div className="space-y-1">
                  <Label>
                    全局密码（可选）
                    {form.password === "***" && (
                      <span className="text-xs text-muted-foreground ml-1">
                        — 已设置
                      </span>
                    )}
                  </Label>
                  <Input
                    type="password"
                    placeholder="代理认证密码"
                    value={form.password}
                    onChange={(e) =>
                      setForm((f) => ({ ...f, password: e.target.value }))
                    }
                  />
                  {form.password === "***" && (
                    <p className="text-xs text-muted-foreground">
                      保持不变请不要改动，清空请全选删除，修改请直接输入新密码
                    </p>
                  )}
                </div>
                <div className="space-y-1">
                  <Label>测试 URL（可选）</Label>
                  <Input
                    placeholder="https://ip.decodo.com/json"
                    value={form.testUrl}
                    onChange={(e) =>
                      setForm((f) => ({ ...f, testUrl: e.target.value }))
                    }
                  />
                </div>
                <div className="space-y-1">
                  <Label>
                    限流冷却（秒）
                    <span className="text-xs text-muted-foreground ml-1">
                      默认 30
                    </span>
                  </Label>
                  <Input
                    type="number"
                    min={1}
                    placeholder="30"
                    value={form.cooldownSecs}
                    onChange={(e) =>
                      setForm((f) => ({ ...f, cooldownSecs: e.target.value }))
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    429 / 503 / 网络错误时代理会进入该冷却期
                  </p>
                </div>
              </div>

              {validationError && (
                <div className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive">
                  {validationError}
                </div>
              )}
            </>
          )}
        </CardContent>
      </Card>

      {/* 展开后的代理列表（含冷却状态） */}
      {proxies.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 flex-wrap">
              <span>当前生效的代理（{proxies.length}）</span>
              {activeCooldownCount > 0 ? (
                <Badge variant="destructive" className="font-normal">
                  <Snowflake className="h-3 w-3 mr-1" />
                  {activeCooldownCount} 个冷却中
                </Badge>
              ) : (
                <Badge className="bg-green-600 hover:bg-green-700 font-normal">
                  全部正常
                </Badge>
              )}
              <span className="text-xs text-muted-foreground font-normal ml-auto">
                冷却时长：{data?.defaultCooldownSecs ?? 30}s
              </span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid gap-2 md:grid-cols-2 lg:grid-cols-3">
              {proxies.map((p, idx) => {
                const remainingMs = Math.max(0, p.cooldownUntilMs - Date.now());
                const inCooldown = remainingMs > 0;
                const remainingSec = Math.ceil(remainingMs / 1000);
                return (
                  <div
                    key={`${idx}-${p.url}`}
                    className={`text-xs p-2 rounded-md border flex items-center gap-2 ${
                      inCooldown
                        ? "border-destructive/40 bg-destructive/5"
                        : "bg-muted"
                    }`}
                    title={p.url}
                  >
                    <span className="font-mono truncate flex-1" title={p.url}>
                      {p.url}
                    </span>
                    {inCooldown ? (
                      <span className="shrink-0 inline-flex items-center gap-1 text-destructive font-medium">
                        <Snowflake className="h-3 w-3" />
                        冷却 {remainingSec}s
                      </span>
                    ) : (
                      <span className="shrink-0 inline-flex items-center gap-1 text-green-600 font-medium">
                        <CheckCircle2 className="h-3 w-3" />
                        正常
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          </CardContent>
        </Card>
      )}

      {/* 测试结果 */}
      {testResults && testSummary && (
        <Card>
          <CardHeader>
            <CardTitle>
              测试结果
              <span className="ml-3 text-sm font-normal text-muted-foreground">
                {testSummary.testUrl}
              </span>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex gap-3 text-sm">
              <Badge className="bg-green-600 hover:bg-green-700">
                成功 {testSummary.success}
              </Badge>
              <Badge variant="destructive">失败 {testSummary.failed}</Badge>
              <Badge variant="secondary">共 {testSummary.total}</Badge>
            </div>
            <div className="grid gap-2">
              {testResults.map((r) => (
                <div
                  key={r.url}
                  className={`flex items-center justify-between gap-2 p-2 rounded-md border text-sm ${
                    r.success
                      ? "border-green-600/30 bg-green-600/5"
                      : "border-destructive/30 bg-destructive/5"
                  }`}
                >
                  <div className="flex items-center gap-2 min-w-0 flex-1">
                    {r.success ? (
                      <CheckCircle2 className="h-4 w-4 text-green-600 flex-shrink-0" />
                    ) : (
                      <XCircle className="h-4 w-4 text-destructive flex-shrink-0" />
                    )}
                    <span className="font-mono text-xs truncate" title={r.url}>
                      {r.url}
                    </span>
                  </div>
                  <div className="flex items-center gap-3 text-xs text-muted-foreground flex-shrink-0">
                    {r.responseIp && (
                      <span className="font-mono">IP: {r.responseIp}</span>
                    )}
                    <span>{r.durationMs}ms</span>
                    {r.error && (
                      <span
                        className="text-destructive truncate max-w-[200px]"
                        title={r.error}
                      >
                        {r.error}
                      </span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
