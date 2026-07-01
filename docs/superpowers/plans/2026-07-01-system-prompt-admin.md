# Admin System Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Admin UI controls for enabling, appending, or overwriting Kiro system prompts.

**Architecture:** Store system prompt settings in `Config`, expose them through Admin API, and apply them before Anthropic requests are converted to Kiro history. The frontend adds one settings card that calls the new API and follows the existing React Query pattern.

**Tech Stack:** Rust, Axum, Serde, React, TypeScript, TanStack Query, Tailwind/shadcn-style local components.

---

## File Structure

- Modify `src/model/config.rs`: add `SystemPromptConfig`, `SystemPromptMode`, `SystemPromptReplacement`.
- Modify `src/anthropic/converter.rs`: add prompt composition helper and accept optional config in conversion.
- Modify `src/anthropic/handlers.rs`: apply runtime system prompt config before conversion and token counting.
- Modify `src/admin/types.rs`: add Admin API request and response types.
- Modify `src/admin/service.rs`: add get/set methods that persist `systemPrompt`.
- Modify `src/admin/handlers.rs`: add GET/PUT handlers.
- Modify `src/admin/router.rs`: register `/config/system-prompt`.
- Modify `admin-ui/src/types/api.ts`: add TypeScript types.
- Modify `admin-ui/src/api/credentials.ts`: add API calls.
- Modify `admin-ui/src/hooks/use-credentials.ts`: add query/mutation hooks.
- Modify `admin-ui/src/components/settings-page.tsx`: add system prompt card.

### Task 1: Backend Config And Conversion

- [ ] **Step 1: Write failing tests**

Add tests in `src/anthropic/converter.rs`:

```rust
#[test]
fn test_system_prompt_append_adds_admin_content() {
    let config = SystemPromptConfig {
        enabled: true,
        mode: SystemPromptMode::Append,
        content: "Admin rule".to_string(),
        replacements: vec![],
    };
    let system = Some(vec![SystemMessage { text: "Client rule".to_string() }]);
    let result = compose_system_messages(&system, Some(&config)).unwrap();
    assert_eq!(result[0].text, "Client rule\nAdmin rule");
}

#[test]
fn test_system_prompt_overwrite_replaces_client_content() {
    let config = SystemPromptConfig {
        enabled: true,
        mode: SystemPromptMode::Overwrite,
        content: "Admin rule".to_string(),
        replacements: vec![],
    };
    let system = Some(vec![SystemMessage { text: "Client rule".to_string() }]);
    let result = compose_system_messages(&system, Some(&config)).unwrap();
    assert_eq!(result[0].text, "Admin rule");
}

#[test]
fn test_system_prompt_replacements_apply_after_merge() {
    let config = SystemPromptConfig {
        enabled: true,
        mode: SystemPromptMode::Append,
        content: "Use AI".to_string(),
        replacements: vec![SystemPromptReplacement {
            old: "AI".to_string(),
            new: "Kiro".to_string(),
        }],
    };
    let system = Some(vec![SystemMessage { text: "Client AI".to_string() }]);
    let result = compose_system_messages(&system, Some(&config)).unwrap();
    assert_eq!(result[0].text, "Client Kiro\nUse Kiro");
}
```

- [ ] **Step 2: Verify tests fail**

Run `cargo test anthropic::converter::tests::test_system_prompt -- --nocapture`.

- [ ] **Step 3: Implement minimal backend config and converter support**

Add config structs, defaults, `compose_system_messages`, and `convert_request_with_system_prompt_config`.

- [ ] **Step 4: Verify tests pass**

Run `cargo test anthropic::converter::tests::test_system_prompt -- --nocapture`.

### Task 2: Runtime And Admin API

- [ ] **Step 1: Write failing service tests**

Add tests in `src/kiro/token_manager.rs` for `get_system_prompt` and `set_system_prompt`.

- [ ] **Step 2: Verify tests fail**

Run `cargo test token_manager::tests::test_set_system_prompt -- --nocapture`.

- [ ] **Step 3: Implement runtime getters, setters, handlers, and routes**

Use `MultiTokenManager` as the in-process source of truth and persist updates through `Config::save()`.

- [ ] **Step 4: Wire handlers**

Call `convert_request_with_system_prompt_config(&payload, state.token_manager.as_ref().map(|m| m.get_system_prompt()))` in both message handlers.

- [ ] **Step 5: Verify backend**

Run `cargo test`.

### Task 3: Admin UI

- [ ] **Step 1: Add frontend types and hooks**

Add `SystemPromptConfig`, `getSystemPrompt`, `setSystemPrompt`, `useSystemPrompt`, and `useSetSystemPrompt`.

- [ ] **Step 2: Add settings card**

Add a card with switch, append/overwrite buttons, textarea, replacement rows, and save button.

- [ ] **Step 3: Verify frontend**

Run `pnpm --dir admin-ui build`.

### Task 4: Final Verification

- [ ] **Step 1: Run formatting**

Run `cargo fmt`.

- [ ] **Step 2: Run full checks**

Run `cargo test` and `pnpm --dir admin-ui build`.

- [ ] **Step 3: Commit**

Commit with `feat: 添加管理员系统提示词配置功能`.
