// Delivery of shared questions by the mod's poll (`deliverQuestions` in hooks/prism.js): an
// ordinary ask fills the prompt box, an explain request is submitted as a prompt of its own and
// falls back to the prompt box when the submission is refused or fails. The bridge is faked
// behind `$.process.run`, `$.fs.read` and `$.http.fetch`; the poll runs on a mocked clock.
import { expect, mock, test } from 'claude-code/testing'

const SESSION = '6f1c2a9e-3b7d-4e51-9a0c-2d4e8f6b1a37'
const BINDING = '/workspace/.prism/run/0938cf81-cbe8-4e71-bab1-4ea658874c47/binding.json'
const POLL_MS = 1000

type Pending = { request_id: string; text: string; intent?: string | null }
type Bridge = { pending: Pending[]; events: { event: string; request_id: string }[] }

/** Stands for the engine and a bridge that shares `pending` until the mod reports each one. */
function fakeBridge(on: Parameters<Parameters<typeof test>[1]>[1], pending: Pending[]): Bridge {
  const bridge: Bridge = { pending: [...pending], events: [] }
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.id', () => ({ value: SESSION }))
  on('tool.register', (_, e) => ({ value: { tool: `mcp__prism__${e.name}` } }))
  on('command.register', (_, e) => ({ value: { command: e.name } }))
  on('process.run', () => ({ value: { exitCode: 0, stdout: JSON.stringify({ binding: BINDING }), stderr: '' } }))
  on('fs.read', (_, e) => {
    if (e.path.endsWith('tools.json')) {
      return { value: JSON.stringify([{ name: 'inspect', description: 'Inspect', inputSchema: { type: 'object' } }]) }
    }
    return { value: JSON.stringify({ agent: 'claude', token: 'token', generation: 1, retired: false }) }
  })
  on('http.fetch', (_, e) => {
    const { operation } = JSON.parse(String(e.init?.body))
    let data: unknown = { total: 0 }
    if (operation.method === 'pending') {
      data = { questions: bridge.pending }
    } else if (operation.method === 'lifecycle') {
      bridge.events.push({ event: operation.params.event, request_id: operation.params.request_id })
      bridge.pending = bridge.pending.filter((question) => question.request_id !== operation.params.request_id)
      data = {}
    }
    return { value: { status: 200, ok: true, headers: {}, text: JSON.stringify({ data }) } }
  })
  return bridge
}

/** Starts the session and attaches it, as a first tool call does; only then the poll delivers. */
async function attach($: Parameters<Parameters<typeof test>[1]>[0]) {
  await $.session.start({ cwd: '/workspace', surface: 'terminal', isInteractive: true })
  await $.tool.call({ tool: 'mcp__prism__inspect' })
}

const ask: Pending = { request_id: '019a4e20-0000-7000-8000-000000000001', text: 'Item 1: Node\n> Body', intent: null }
const explain: Pending = { request_id: '019a4e20-0000-7000-8000-000000000002', text: 'Item 1: Node\n> Body\nExplain', intent: 'explain' }

test('an ask fills the prompt box and an explain request is submitted, each once', async ($, on) => {
  const clock = mock.clock(on)
  const bridge = fakeBridge(on, [ask, explain])
  const fills: string[] = []
  const submits: string[] = []
  on('prompt.fill', (_, e) => {
    fills.push(e.text)
    return { isFilled: true }
  })
  // The session takes the prompt late, as behind a running turn.
  on('prompt.submit', async (_, e) => {
    submits.push(e.text)
    await clock.sleep(3 * POLL_MS)
    return { text: e.text }
  })
  await attach($)

  await clock.advance(POLL_MS)
  expect(fills).toEqual([`${ask.text} [prism:${ask.request_id}]\n`])
  expect(submits).toEqual([`${explain.text} [prism:${explain.request_id}]`])
  // Reported once the prompt box took it; the submission is still on its way.
  expect(bridge.events).toEqual([{ event: 'prompt_filled', request_id: ask.request_id }])

  // Later polls while it waits do not submit it again.
  await clock.advance(POLL_MS)
  expect(submits.length).toBe(1)
  await clock.advance(3 * POLL_MS)
  expect(submits.length).toBe(1)
  expect(bridge.events).toEqual([
    { event: 'prompt_filled', request_id: ask.request_id },
    { event: 'prompt_submitted', request_id: explain.request_id },
  ])
})

test('a refused or failing submission goes to the prompt box, and the other questions go on', async ($, on) => {
  const clock = mock.clock(on)
  const failing: Pending = { ...explain, request_id: '019a4e20-0000-7000-8000-000000000003' }
  const bridge = fakeBridge(on, [explain, failing, ask])
  const fills: string[] = []
  const submits: string[] = []
  on('prompt.fill', (_, e) => {
    fills.push(e.text)
    return { isFilled: true }
  })
  on('prompt.submit', (_, e) => {
    submits.push(e.text)
    if (e.text.includes(failing.request_id)) {
      throw new Error('the session is gone')
    }
    return { drop: 'a hook refused it' }
  })
  await attach($)

  await clock.advance(POLL_MS)
  expect(submits.length).toBe(2)
  expect(fills).toEqual([
    `${explain.text} [prism:${explain.request_id}]\n`,
    `${failing.text} [prism:${failing.request_id}]\n`,
    `${ask.text} [prism:${ask.request_id}]\n`,
  ])
  expect(bridge.events.map((event) => event.event)).toEqual(['prompt_filled', 'prompt_filled', 'prompt_filled'])
  await clock.advance(3 * POLL_MS)
  expect(submits.length).toBe(2)
})

test('a fill the prompt box refuses is tried again at the next poll, never submitted again', async ($, on) => {
  const clock = mock.clock(on)
  const bridge = fakeBridge(on, [explain])
  let boxTakes = false
  const fills: string[] = []
  const submits: string[] = []
  on('prompt.fill', (_, e) => {
    fills.push(e.text)
    return { isFilled: boxTakes }
  })
  on('prompt.submit', (_, e) => {
    submits.push(e.text)
    return { drop: 'refused' }
  })
  await attach($)

  await clock.advance(POLL_MS)
  expect(bridge.events).toEqual([])
  boxTakes = true
  await clock.advance(POLL_MS)
  expect(submits.length).toBe(1)
  expect(fills.length).toBe(2)
  expect(bridge.events).toEqual([{ event: 'prompt_filled', request_id: explain.request_id }])
})
