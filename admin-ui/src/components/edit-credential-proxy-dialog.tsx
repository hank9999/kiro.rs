import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useUpdateCredentialProxy } from "@/hooks/use-proxy";
import type { CredentialStatusItem } from "@/types/api";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  credential: CredentialStatusItem;
}

type Mode = "inherit" | "direct" | "custom";

export function EditCredentialProxyDialog({
  open,
  onOpenChange,
  credential,
}: Props) {
  const { mutateAsync, isPending } = useUpdateCredentialProxy();

  const initialMode: Mode = !credential.proxyUrl
    ? "inherit"
    : credential.proxyUrl.trim().toLowerCase() === "direct"
      ? "direct"
      : "custom";

  const [mode, setMode] = useState<Mode>(initialMode);
  // 当前展示值为已屏蔽的 URL（例如 socks5h://user:***@host），
  // 不能原样提交回后端，初始留空提示用户重新输入完整 URL
  const [proxyUrl, setProxyUrl] = useState<string>("");
  const [proxyUsername, setProxyUsername] = useState("");
  const [proxyPassword, setProxyPassword] = useState("");

  useEffect(() => {
    if (open) {
      const m: Mode = !credential.proxyUrl
        ? "inherit"
        : credential.proxyUrl.trim().toLowerCase() === "direct"
          ? "direct"
          : "custom";
      setMode(m);
      setProxyUrl("");
      setProxyUsername("");
      setProxyPassword("");
    }
  }, [open, credential]);

  const handleSave = async () => {
    try {
      const payload: {
        proxyUrl?: string | null;
        proxyUsername?: string | null;
        proxyPassword?: string | null;
      } = {};

      if (mode === "inherit") {
        payload.proxyUrl = null;
        payload.proxyUsername = null;
        payload.proxyPassword = null;
      } else if (mode === "direct") {
        payload.proxyUrl = "direct";
        payload.proxyUsername = null;
        payload.proxyPassword = null;
      } else {
        const url = proxyUrl.trim();
        if (!url) {
          toast.error("请填写代理 URL");
          return;
        }
        payload.proxyUrl = url;
        payload.proxyUsername = proxyUsername.trim() || null;
        payload.proxyPassword = proxyPassword || null;
      }

      await mutateAsync({ id: credential.id, payload });
      toast.success(`凭据 #${credential.id} 代理已更新`);
      onOpenChange(false);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(`更新失败: ${msg}`);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[520px]">
        <DialogHeader>
          <DialogTitle>编辑凭据 #{credential.id} 代理</DialogTitle>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label>代理模式</Label>
            <div className="grid grid-cols-3 gap-2">
              <ModeCard
                active={mode === "inherit"}
                onClick={() => setMode("inherit")}
                title="继承"
                desc="使用代理池 / 全局代理"
              />
              <ModeCard
                active={mode === "direct"}
                onClick={() => setMode("direct")}
                title="直连"
                desc="强制绕过所有代理"
              />
              <ModeCard
                active={mode === "custom"}
                onClick={() => setMode("custom")}
                title="自定义"
                desc="仅此凭据使用指定代理"
              />
            </div>
          </div>

          {mode === "custom" && (
            <div className="space-y-3 rounded-md border p-3">
              {credential.proxyUrl &&
                credential.proxyUrl.trim().toLowerCase() !== "direct" && (
                  <div className="rounded-md bg-blue-500/10 border border-blue-500/30 p-2 text-xs">
                    当前已设置：
                    <span className="font-mono ml-1">{credential.proxyUrl}</span>
                    <div className="text-muted-foreground mt-1">
                      出于安全考虑密码已屏蔽，如需保持不变请关闭对话框
                    </div>
                  </div>
                )}
              <div className="space-y-1">
                <Label>代理 URL</Label>
                <Input
                  placeholder="socks5h://host:10001 或 http://host:8080"
                  value={proxyUrl}
                  onChange={(e) => setProxyUrl(e.target.value)}
                />
              </div>
              <div className="grid gap-3 md:grid-cols-2">
                <div className="space-y-1">
                  <Label>用户名（可选）</Label>
                  <Input
                    placeholder="代理认证用户名"
                    value={proxyUsername}
                    onChange={(e) => setProxyUsername(e.target.value)}
                  />
                </div>
                <div className="space-y-1">
                  <Label>密码（可选，留空保持不变）</Label>
                  <Input
                    type="password"
                    placeholder="留空表示不修改密码"
                    value={proxyPassword}
                    onChange={(e) => setProxyPassword(e.target.value)}
                  />
                </div>
              </div>
              <p className="text-xs text-muted-foreground">
                若 URL 本身带认证（例如 user:pass@host），下方认证字段可留空
              </p>
            </div>
          )}

          <div className="rounded-md bg-muted/50 p-3 text-xs text-muted-foreground space-y-1">
            <div>
              <span className="font-medium text-foreground">优先级：</span>
              凭据级代理 &gt; 代理池 &gt; 全局单代理 &gt; 直连
            </div>
            <div>选择"继承"即由全局规则决定该凭据使用哪个代理</div>
          </div>
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isPending}
          >
            取消
          </Button>
          <Button onClick={handleSave} disabled={isPending}>
            {isPending ? "保存中..." : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ModeCard({
  active,
  onClick,
  title,
  desc,
}: {
  active: boolean;
  onClick: () => void;
  title: string;
  desc: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-md border p-3 text-left transition-colors hover:bg-accent ${
        active
          ? "border-primary bg-primary/5"
          : "border-border bg-background"
      }`}
    >
      <div className="font-medium text-sm">{title}</div>
      <div className="text-xs text-muted-foreground mt-1">{desc}</div>
    </button>
  );
}
