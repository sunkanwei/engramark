#!/usr/bin/env node
import assert from "node:assert/strict"
import { access, mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { spawnSync } from "node:child_process"
import {
  BLOCK_END,
  BLOCK_START,
  buildBlock,
  createEngramarkPlugin,
  createRunJson,
  extractDirectText,
  extractSingleBlock,
  memoryBlockState,
  mergeMemoryBlock,
  validateScanResponse,
  verifyOpenCode,
} from "../adapters/opencode/engramark.js"

const ROOT = resolve(import.meta.dirname, "..")
const BINARY = join(ROOT, "rust", "target", "debug",
  process.platform === "win32" ? "engramark.exe" : "engramark")
let passed = 0

const test = async (name, fn) => {
  await fn()
  passed += 1
  process.stdout.write(`  PASS ${name}\n`)
}

const core = (home, args, input = "") => {
  const result = spawnSync(BINARY, args, {
    input, encoding: "utf8", env: { ...process.env, ENGRAMARK_HOME: home }, timeout: 30_000,
  })
  assert.equal(result.status, 0, result.stderr)
  return JSON.parse(result.stdout)
}

const card = `@0 fact published I3 T3 2026-08-01
= OrchidUI, core
~ user
# lock
OrchidUI（口头称 core）是示例扩展。
`

await test("只提取真实文本并安全截断", async () => {
  assert.equal(extractDirectText([
    { type: "text", text: "  first  " },
    { type: "text", text: "synthetic", synthetic: true },
    { type: "text", text: "ignored", ignored: true },
    { type: "file", text: "file" },
    { type: "text", text: "second" },
  ]), "first\nsecond")
  assert.equal([...extractDirectText([{ type: "text", text: "😀".repeat(5000) }])].length, 4096)
})

await test("系统块合并、冲突和单块提取", async () => {
  const block = buildBlock(["记忆提示：记忆 1：OrchidUI"])
  assert.equal(memoryBlockState(undefined), "absent")
  assert.equal(memoryBlockState(block), "complete")
  assert.equal(memoryBlockState(BLOCK_START), "conflict")
  const malformed = `${BLOCK_START}\nnot an index line\n${BLOCK_END}`
  assert.equal(memoryBlockState(malformed), "conflict")
  assert.equal(extractSingleBlock(malformed), null)
  assert.equal(mergeMemoryBlock("keep", block).value, `keep\n\n${block}`)
  assert.equal(extractSingleBlock(`keep\n${block}`), block)
  assert.equal(extractSingleBlock(`${block}\n${block}`), null)
})

await test("响应校验拒绝重复、控制字符和超限", async () => {
  const base = { protocol_version: 1, status: "ok", reservation_id: "a".repeat(24),
    session_key: "b".repeat(64) }
  assert.equal(validateScanResponse({ ...base,
    items: [{ id: 1, line: "记忆提示：记忆 1：OrchidUI" }] }).ok, true)
  assert.equal(validateScanResponse({ ...base, items: [
    { id: 1, line: "记忆提示：记忆 1：A" }, { id: 1, line: "记忆提示：记忆 1：B" },
  ] }).ok, false)
  assert.equal(validateScanResponse({ ...base,
    items: [{ id: 1, line: "记忆提示：记忆 1：bad\u0000" }] }).ok, false)
  assert.equal(validateScanResponse({ ...base,
    items: [{ id: Number.MAX_SAFE_INTEGER + 1,
      line: `记忆提示：记忆 ${Number.MAX_SAFE_INTEGER + 1}：unsafe` }] }).ok, false)
  assert.equal(validateScanResponse({ ...base,
    items: [{ id: 1, line: `记忆提示：记忆 1：${"😀".repeat(220)}` }] }).ok, false)
  assert.equal(validateScanResponse({ protocol_version: 1, status: "ok", items: [],
    reservation_id: "a".repeat(24) }).ok, false)
  assert.equal(validateScanResponse({ protocol_version: 1, status: "unavailable", items: [],
    reason: "cache_busy", reservation_id: "a".repeat(24), session_key: "b".repeat(64) }).ok,
  false)
})

await test("健康门只接受精确 1.18.11", async () => {
  const response = (version) => async () => ({ ok: true, text: async () => JSON.stringify({
    healthy: true, version,
  }) })
  assert.equal(await verifyOpenCode({ serverUrl: "http://127.0.0.1:1" }, response("1.18.11")), true)
  assert.equal(await verifyOpenCode({ serverUrl: "http://127.0.0.1:1" }, response("1.18.12")), false)
  let authenticatedCalls = 0
  const authenticated = (version) => ({ client: { _client: { get: async (options) => {
    authenticatedCalls += 1
    assert.equal(options.url, "/global/health")
    return { response: { ok: true }, data: JSON.stringify({ healthy: true, version }) }
  } } } })
  assert.equal(await verifyOpenCode(authenticated("1.18.11"), () => {
    throw new Error("不应绕过 OpenCode SDK 的鉴权客户端")
  }), true)
  assert.equal(await verifyOpenCode(authenticated("1.18.12")), false)
  assert.equal(authenticatedCalls, 2)

  const disabled = await createEngramarkPlugin({
    config: { enabled: true, budget: 3, allowUnverified: false },
    verified: false,
  })({ directory: "/tmp/project" })
  assert.deepEqual(disabled, {})
  let warnings = 0
  const allowed = await createEngramarkPlugin({
    config: { enabled: true, budget: 3, allowUnverified: true },
    verified: false,
    runJson: async () => ({ ok: true, value: {} }),
  })({ directory: "/tmp/project", client: { app: { log: async () => { warnings += 1 } } } })
  assert.equal(typeof allowed["chat.message"], "function")
  await new Promise((resolvePromise) => setTimeout(resolvePromise, 0))
  assert.equal(warnings, 1)
  await allowed.dispose()
})

await test("子进程封装有界、超时且失败开放", async () => {
  const directory = await mkdtemp(join(tmpdir(), "engramark-run-json-"))
  const active = new Set()
  try {
    const script = join(directory, "child.mjs")
    await writeFile(script, `
const mode = process.argv[2]
process.stdin.resume()
if (mode === "ok") process.stdout.write(JSON.stringify({ ok: true }))
else if (mode === "large") process.stdout.write("x".repeat(17 * 1024))
else if (mode === "bad-utf8") process.stdout.write(Buffer.from([0xff]))
else if (mode === "nonzero") process.exitCode = 7
else if (mode === "hang") {
  process.on("SIGTERM", () => {})
  setInterval(() => {}, 1000)
}
`, "utf8")
    const runJson = createRunJson({ binary: process.execPath, active })
    assert.deepEqual((await runJson([script, "ok"], {}, Date.now() + 1000)).value, { ok: true })
    assert.equal((await runJson([script, "large"], {}, Date.now() + 1000)).reason, "output")
    assert.equal((await runJson([script, "bad-utf8"], {}, Date.now() + 1000)).reason, "json")
    assert.equal((await runJson([script, "nonzero"], {}, Date.now() + 1000)).reason, "exit")
    assert.equal((await runJson([script, "ok"], { text: "x".repeat(33 * 1024) },
      Date.now() + 1000)).reason, "input")
    const timeout = await runJson([script, "hang"], {}, Date.now() + 140)
    assert.equal(timeout.reason, "timeout")
    assert.ok(timeout.durationMs < 400)
    assert.equal((await createRunJson({ binary: join(directory, "missing") })(
      [], {}, Date.now() + 1000)).reason, "spawn")
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 20))
    assert.equal(active.size, 0)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

await test("插件命令、消息 ID、耐久提交和改写取消", async () => {
  const calls = []
  let serial = 0
  const runJson = async (args) => {
    calls.push(args[0])
    if (args[0] === "scan") {
      serial += 1
      return { ok: true, value: { protocol_version: 1, status: "ok",
        items: [{ id: serial, line: `记忆提示：记忆 ${serial}：Item` }],
        reservation_id: `reservation_${String(serial).padStart(12, "0")}`,
        session_key: "c".repeat(64) } }
    }
    return { ok: true, value: { protocol_version: 1, status: "ok", applied: true } }
  }
  const factory = createEngramarkPlugin({ config: { enabled: true, budget: 3 },
    verified: true, runJson })
  const hooks = await factory({ directory: "/tmp/project", worktree: "/tmp/project" })
  await hooks.event()
  await hooks["chat.message"]({}, Object.freeze({}))
  await hooks["chat.message"]({ sessionID: "s", messageID: "wrong" }, {
    message: { id: "m0", sessionID: "s", role: "user" }, parts: [{ type: "text", text: "core" }],
  })
  assert.equal(calls.filter((name) => name === "scan").length, 0)
  await hooks["command.execute.before"]({ sessionID: "s", command: "x", arguments: "core" }, {})
  await hooks["chat.message"]({ sessionID: "s", messageID: "m1" }, {
    message: { id: "m1", sessionID: "s", role: "user" }, parts: [{ type: "text", text: "core" }],
  })
  assert.equal(calls.filter((name) => name === "scan").length, 0)
  const existingBlock = buildBlock(["记忆提示：记忆 99：已有索引"])
  for (const [id, system] of [["existing", existingBlock], ["conflict", BLOCK_START]]) {
    await hooks["chat.message"]({ sessionID: "s", messageID: id }, {
      message: { id, sessionID: "s", role: "user", system },
      parts: [{ type: "text", text: "core" }],
    })
  }
  assert.equal(calls.filter((name) => name === "scan").length, 0)
  await hooks["command.execute.before"]({ sessionID: "s", command: "x", arguments: "core" }, {})
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s" } } })
  const message = { id: "m2", sessionID: "s", role: "user", system: "keep" }
  await hooks["chat.message"]({ sessionID: "s", messageID: "m2" }, {
    message, parts: [{ type: "text", text: "core" }],
  })
  assert.match(message.system, /keep\n\n\[long-term-memory-index:v1\]/)
  await hooks.event({ event: { type: "message.updated", properties: { info: message } } })
  assert.equal(calls.at(-1), "scan-commit")

  const overwritten = { id: "m3", sessionID: "s", role: "user" }
  await hooks["chat.message"]({ sessionID: "s", messageID: "m3" }, {
    message: overwritten, parts: [{ type: "text", text: "core" }],
  })
  overwritten.system = "removed by later plugin"
  await hooks.event({ event: { type: "message.updated", properties: { info: overwritten } } })
  assert.equal(calls.at(-1), "scan-cancel")

  const orphaned = { id: "m4", sessionID: "s", role: "user" }
  await hooks["chat.message"]({ sessionID: "s", messageID: "m4" }, {
    message: orphaned, parts: [{ type: "text", text: "core" }],
  })
  const cancelsBeforeIdle = calls.filter((name) => name === "scan-cancel").length
  await hooks.event({ event: { type: "session.idle", properties: { sessionID: "s" } } })
  assert.equal(calls.filter((name) => name === "scan-cancel").length, cancelsBeforeIdle)

  const disposePending = { id: "m5", sessionID: "s", role: "user" }
  await hooks["chat.message"]({ sessionID: "s", messageID: "m5" }, {
    message: disposePending, parts: [{ type: "text", text: "core" }],
  })
  const cancelsBeforeDispose = calls.filter((name) => name === "scan-cancel").length
  await hooks.dispose()
  assert.equal(calls.filter((name) => name === "scan-cancel").length, cancelsBeforeDispose)
})

await test("非法响应尽力取消保留且异常不逃逸", async () => {
  const calls = []
  const runJson = async (args) => {
    calls.push(args[0])
    if (args[0] === "scan") return { ok: true, value: {
      protocol_version: 1, status: "ok",
      items: [{ id: 1, line: "记忆提示：记忆 1：bad\nline" }],
      reservation_id: "reservation_invalid_001", session_key: "d".repeat(64),
    } }
    throw new Error("控制进程故障")
  }
  const hooks = await createEngramarkPlugin({ config: { enabled: true, budget: 3 },
    verified: true, runJson })({ directory: "/tmp/project", worktree: "/tmp/project" })
  const message = { id: "invalid", sessionID: "safe", role: "user" }
  await hooks["chat.message"]({ sessionID: "safe", messageID: "invalid" }, {
    message, parts: [{ type: "text", text: "core" }],
  })
  assert.equal(message.system, undefined)
  assert.deepEqual(calls.slice(-2), ["scan", "scan-cancel"])
  await hooks.dispose()
})

await test("真实内核端到端不保存会话内容且冷却生效", async () => {
  const home = await mkdtemp(join(tmpdir(), "engramark-opencode-"))
  const workspaceParent = await mkdtemp(join(tmpdir(), "engramark-workspace-"))
  const workspace = join(workspaceParent, "OrchidUI")
  try {
    await mkdir(workspace)
    await mkdir(join(workspace, ".git"))
    core(home, ["save", card, "--lock"])
    const factory = createEngramarkPlugin({ config: { enabled: true, budget: 3 }, verified: true,
      binary: BINARY, dataHome: home })
    const hooks = await factory({ directory: workspace, worktree: workspace })
    const message = { id: "real-m1", sessionID: "real-s1", role: "user" }
    await hooks["chat.message"]({ sessionID: "real-s1", messageID: "real-m1" }, {
      message, parts: [{ type: "text", text: "帮我修改 core" }],
    })
    assert.match(message.system, /记忆提示：记忆 1：/)
    await hooks.event({ event: { type: "message.updated", properties: { info: message } } })
    const second = { id: "real-m2", sessionID: "real-s1", role: "user" }
    await hooks["chat.message"]({ sessionID: "real-s1", messageID: "real-m2" }, {
      message: second, parts: [{ type: "text", text: "再看 core" }],
    })
    assert.equal(second.system, undefined)
    await hooks.dispose()
    const stateDirectory = join(home, "cache", "radar-state")
    const stateFiles = (await readdir(stateDirectory)).filter((name) => name.startsWith("hook-"))
    assert.equal(stateFiles.length, 1)
    const stateText = await readFile(join(stateDirectory, stateFiles[0]), "utf8")
    assert.doesNotMatch(stateText, /帮我修改|再看 core|OrchidUI/)
    for (const forbidden of [join(home, "raw"), join(home, "state", "distill"),
      join(home, "state", "jobs")]) {
      await assert.rejects(access(forbidden))
    }
  } finally {
    await rm(home, { recursive: true, force: true })
    await rm(workspaceParent, { recursive: true, force: true })
  }
})

process.stdout.write(`\n结果：${passed} 通过 / 0 失败\n`)
