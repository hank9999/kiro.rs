import { Activity, Bot, FileText, RefreshCw, ChevronDown, ChevronUp } from "lucide-react";
import { useState, useRef, useEffect, useCallback } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  useAvailableModels,
  useRecentLogs,
  useRequestActivity,
} from "@/hooks/use-credentials";
import type { RequestActivityRecord } from "@/types/api";

function formatTime(value: string | null | undefined) {
  if (!value) {
    return "-";
  }

  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  return date.toLocaleTimeString("zh-CN", {
    hour12: false,
  });
}

function formatDuration(durationMs: number) {
  if (durationMs < 1000) {
    return `${durationMs}ms`;
  }

  return `${(durationMs / 1000).toFixed(2)}s`;
}

function formatTokenCount(maxTokens: number | undefined) {
  if (!maxTokens) {
    return "-";
  }
  return maxTokens.toLocaleString("zh-CN");
}

function getStatusVariant(success: boolean, statusCode: number) {
  if (success) {
    return "success" as const;
  }

  if (statusCode >= 500) {
    return "destructive" as const;
  }

  return "warning" as const;
}

function getLogLineClass(line: string) {
  if (line.includes(" ERROR ")) {
    return "text-red-500";
  }

  if (line.includes(" WARN ")) {
    return "text-yellow-500";
  }

  if (line.includes(" INFO ")) {
    return "text-foreground";
  }

  return "text-muted-foreground";
}

function hasDiagnostics(record: RequestActivityRecord) {
  return Boolean(
    record.error ||
    record.model ||
    record.clientIp ||
    record.forwardedFor ||
    record.realIp ||
    record.forwardedProto ||
    record.userAgent ||
    record.referer ||
    record.origin ||
    record.transferEncoding ||
    typeof record.contentLength === "number" ||
    record.clientRequestId,
  );
}

function LogViewer({
  lines,
  containerRef,
  autoScroll,
  onScroll,
}: {
  lines: string[];
  containerRef: React.RefObject<HTMLDivElement>;
  autoScroll: boolean;
  onScroll: () => void;
}) {
  useEffect(() => {
    if (autoScroll && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [lines, autoScroll, containerRef]);

  return (
    <div
      ref={containerRef}
      onScroll={onScroll}
      className="h-[28rem] overflow-auto rounded-lg border bg-muted/20 p-3 font-mono text-xs leading-5"
    >
      {lines.length === 0 ? (
        <div className="text-muted-foreground">暂无日志内容</div>
      ) : (
        lines.map((line, index) => (
          <div
            key={`${index}-${line.slice(0, 32)}`}
            className={getLogLineClass(line)}
          >
            {line}
          </div>
        ))
      )}
    </div>
  );
}

export function ActivityMonitor() {
  const [expandedRecords, setExpandedRecords] = useState<Set<number>>(new Set());
  const [logAutoScroll, setLogAutoScroll] = useState(true);
  const logContainerRef = useRef<HTMLDivElement>(null);

  const handleLogScroll = useCallback(() => {
    const el = logContainerRef.current;
    if (!el) return;
    const isAtBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setLogAutoScroll(isAtBottom);
  }, []);

  const toggleRecord = (id: number) => {
    setExpandedRecords((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const {
    data: activity,
    isLoading: loadingActivity,
    isFetching: fetchingActivity,
    refetch: refetchActivity,
  } = useRequestActivity(20);
  const {
    data: availableModels,
    error: availableModelsError,
    isLoading: loadingModels,
    isFetching: fetchingModels,
    refetch: refetchModels,
  } = useAvailableModels();
  const {
    data: logs,
    isLoading: loadingLogs,
    isFetching: fetchingLogs,
    refetch: refetchLogs,
  } = useRecentLogs(120);

  const records = activity?.records ?? [];
  const summary = activity?.summary;
  const models = availableModels?.data ?? [];

  const handleRefresh = () => {
    void refetchModels();
    void refetchActivity();
    void refetchLogs();
  };

  return (
    <div className="mb-6 space-y-6">
      <Card>
        <CardHeader className="flex flex-row items-start justify-between space-y-0">
          <div>
            <CardTitle className="flex items-center gap-2 text-lg">
              <Bot className="h-5 w-5" />
              可用模型
            </CardTitle>
            <CardDescription>
              展示当前服务暴露给 OpenAI / Anthropic 兼容接口的模型列表
            </CardDescription>
          </div>
          <Badge variant="secondary">共 {models.length} 个</Badge>
        </CardHeader>
        <CardContent>
          {loadingModels ? (
            <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
              正在加载模型列表...
            </div>
          ) : availableModelsError ? (
            <div className="rounded-lg border border-dashed p-6 text-sm text-red-500">
              模型列表加载失败
            </div>
          ) : models.length === 0 ? (
            <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
              当前没有可展示的模型
            </div>
          ) : (
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {models.map((model) => (
                <div key={model.id} className="rounded-lg border p-3">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="font-medium">{model.displayName}</div>
                      <div className="mt-1 break-all font-mono text-xs text-muted-foreground">
                        {model.id}
                      </div>
                    </div>
                    <Badge variant="outline">
                      {formatTokenCount(model.maxTokens)} tokens
                    </Badge>
                  </div>
                  <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                    <Badge variant="secondary">{model.ownedBy}</Badge>
                    <span>{model.type}</span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-6 xl:grid-cols-[1.15fr_0.85fr]">
        <Card>
          <CardHeader className="flex flex-row items-start justify-between space-y-0">
            <div>
              <CardTitle className="flex items-center gap-2 text-lg">
                <Activity className="h-5 w-5" />
                实时调用记录
              </CardTitle>
              <CardDescription>
                每 3 秒自动刷新，请求成功/失败会直接体现在这里
              </CardDescription>
            </div>
            <Button variant="outline" size="sm" onClick={handleRefresh}>
              <RefreshCw
                className={`h-4 w-4 mr-2 ${
                  fetchingModels || fetchingActivity || fetchingLogs
                    ? "animate-spin"
                    : ""
                }`}
              />
              刷新
            </Button>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-4">
              <div className="rounded-lg border bg-muted/30 p-3">
                <div className="text-xs text-muted-foreground">总调用</div>
                <div className="mt-1 text-2xl font-semibold">
                  {summary?.totalRequests ?? 0}
                </div>
              </div>
              <div className="rounded-lg border bg-green-500/10 p-3">
                <div className="text-xs text-muted-foreground">成功</div>
                <div className="mt-1 text-2xl font-semibold text-green-600">
                  {summary?.successRequests ?? 0}
                </div>
              </div>
              <div className="rounded-lg border bg-red-500/10 p-3">
                <div className="text-xs text-muted-foreground">失败</div>
                <div className="mt-1 text-2xl font-semibold text-red-600">
                  {summary?.failedRequests ?? 0}
                </div>
              </div>
              <div className="rounded-lg border bg-blue-500/10 p-3">
                <div className="text-xs text-muted-foreground">进行中</div>
                <div className="mt-1 text-2xl font-semibold text-blue-600">
                  {summary?.inFlight ?? 0}
                </div>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
              <Badge variant="secondary">
                成功率 {summary ? `${summary.successRate.toFixed(1)}%` : "0.0%"}
              </Badge>
              <span>最后更新 {formatTime(summary?.lastUpdatedAt)}</span>
            </div>

            <div className="space-y-2">
              {loadingActivity ? (
                <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
                  正在加载最近调用记录...
                </div>
              ) : records.length === 0 ? (
                <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
                  还没有调用记录
                </div>
              ) : (
                records.map((record) => {
                  const isExpanded = expandedRecords.has(record.id);
                  const showDiagnostics = hasDiagnostics(record);

                  return (
                    <div
                      key={record.id}
                      className="grid gap-3 rounded-lg border p-3 md:grid-cols-[auto_1fr_auto]"
                    >
                      <div className="flex items-start gap-2">
                        <Badge
                          variant={getStatusVariant(
                            record.success,
                            record.statusCode,
                          )}
                        >
                          {record.statusCode}
                        </Badge>
                        <Badge variant="outline">{record.method}</Badge>
                        {record.stream && (
                          <Badge variant="secondary">stream</Badge>
                        )}
                      </div>

                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-medium">{record.endpoint}</span>
                          <span className="font-mono text-xs text-muted-foreground break-all">
                            {record.path}
                          </span>
                        </div>
                        <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                          <Badge variant="secondary">{record.requestId}</Badge>
                          {record.model && (
                            <Badge variant="outline" className="font-normal">
                              {record.model}
                            </Badge>
                          )}
                          {typeof record.messageCount === "number" && (
                            <span>{record.messageCount} 条消息</span>
                          )}
                          {showDiagnostics && (
                            <button
                              onClick={() => toggleRecord(record.id)}
                              className="flex items-center gap-1 text-blue-600 hover:text-blue-700 hover:underline"
                            >
                              {isExpanded ? (
                                <>
                                  <ChevronUp className="h-3 w-3" />
                                  收起详情
                                </>
                              ) : (
                                <>
                                  <ChevronDown className="h-3 w-3" />
                                  展开详情
                                </>
                              )}
                            </button>
                          )}
                        </div>
                        {showDiagnostics && isExpanded && (
                          <div className="mt-2 space-y-1 text-xs text-muted-foreground">
                            {record.error && (
                              <div className="break-all text-red-500">
                                {record.error}
                              </div>
                            )}
                            {record.clientRequestId && (
                              <div className="break-all">
                                客户端请求 ID: {record.clientRequestId}
                              </div>
                            )}
                            {record.clientIp && (
                              <div className="break-all">
                                来源 IP: {record.clientIp}
                              </div>
                            )}
                            {record.realIp && (
                              <div className="break-all">
                                真实 IP: {record.realIp}
                              </div>
                            )}
                            {record.forwardedFor && (
                              <div className="break-all">
                                X-Forwarded-For: {record.forwardedFor}
                              </div>
                            )}
                            {record.forwardedProto && (
                              <div className="break-all">
                                X-Forwarded-Proto: {record.forwardedProto}
                              </div>
                            )}
                            {typeof record.contentLength === "number" && (
                              <div>Content-Length: {record.contentLength}</div>
                            )}
                            {record.transferEncoding && (
                              <div className="break-all">
                                Transfer-Encoding: {record.transferEncoding}
                              </div>
                            )}
                            {record.origin && (
                              <div className="break-all">
                                Origin: {record.origin}
                              </div>
                            )}
                            {record.referer && (
                              <div className="break-all">
                                Referer: {record.referer}
                              </div>
                            )}
                            {record.userAgent && (
                              <div className="break-all">
                                User-Agent: {record.userAgent}
                              </div>
                            )}
                          </div>
                        )}
                      </div>

                      <div className="text-left text-xs text-muted-foreground md:text-right">
                        <div>{formatTime(record.finishedAt)}</div>
                        <div>{formatDuration(record.durationMs)}</div>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-lg">
              <FileText className="h-5 w-5" />
              运行日志
            </CardTitle>
            <CardDescription>
              展示最近 120 行 `kiro.log`，错误和警告会高亮
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="mb-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
              <Badge variant={logs?.available ? "success" : "warning"}>
                {logs?.available ? "日志可读" : "日志不可用"}
              </Badge>
              {logs?.truncated && <Badge variant="secondary">仅展示尾部</Badge>}
              <span>{logs?.path ?? "kiro.log"}</span>
              {!logAutoScroll && (
                <button
                  onClick={() => {
                    setLogAutoScroll(true);
                    logContainerRef.current?.scrollTo({ top: logContainerRef.current.scrollHeight, behavior: "smooth" });
                  }}
                  className="text-xs text-blue-600 hover:text-blue-700 hover:underline"
                >
                  跳到底部
                </button>
              )}
            </div>

            {loadingLogs ? (
              <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
                正在加载日志...
              </div>
            ) : logs && !logs.available ? (
              <div className="rounded-lg border border-dashed p-6 text-sm text-red-500">
                {logs.error || "日志文件不可用"}
              </div>
            ) : (
              <LogViewer
                lines={logs?.lines ?? []}
                containerRef={logContainerRef}
                autoScroll={logAutoScroll}
                onScroll={handleLogScroll}
              />
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
