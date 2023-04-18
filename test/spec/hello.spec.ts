import 'jest'
import { nanoid } from 'nanoid'
import { sign } from 'jsonwebtoken'
import { ContentType } from 'allure-js-commons'

import { FunctionsClient } from '@supabase/functions-js'

import { Relay, runRelay } from '../relay/container'
import { attach, log } from '../utils/jest-custom-reporter'
import { getCustomFetch } from '../utils/fetch'

const port = 9000

describe('basic tests (hello function)', () => {
  let relay: Relay
  const jwtSecret = nanoid(10)
  const apiKey = sign({ name: 'anon' }, jwtSecret)

  beforeAll(async () => {
    relay = await runRelay({})
  })

  afterAll(async () => {
    relay && relay.container && (await relay.container.stop())
  })

  test('invoke hello with auth header', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://0.0.0.0:${relay.container.getMappedPort(port)}`, {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('hello', {})

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  test('invoke hello with setAuth', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`)
    attach('setAuth', apiKey, ContentType.TEXT)
    fclient.setAuth(apiKey)

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('hello', {})

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  // no jwt support yet
  test.skip('invoke hello with setAuth wrong key', async () => {
    /**
     * @feature errors
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`)
    const wrongKey = sign({ name: 'anon' }, 'wrong_jwt')
    attach('setAuth with wrong jwt', wrongKey, ContentType.TEXT)
    fclient.setAuth(wrongKey)

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('hello', {})

    log('check error')
    expect(error).not.toBeNull()
    expect(error?.message).toEqual('Relay Error invoking the Edge Function')
    expect(data).toBeNull()
  })

  // no jwt support yet
  test.skip('invoke hello: auth override by setAuth wrong key', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`, {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })
    const wrongKey = sign({ name: 'anon' }, 'wrong_jwt')
    attach('setAuth with wrong jwt', wrongKey, ContentType.TEXT)
    fclient.setAuth(wrongKey)

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('hello', {})

    log('check error')
    expect(error).not.toBeNull()
    expect(error?.message).toEqual('Relay Error invoking the Edge Function')
    expect(data).toBeNull()
  })

  test('invoke hello: auth override by setAuth right key', async () => {
    /**
     * @feature auth
     */
    const wrongKey = sign({ name: 'anon' }, 'wrong_jwt')

    log('create FunctionsClient with wrong jwt')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`, {
      headers: {
        Authorization: `Bearer ${wrongKey}`,
      },
    })

    attach('setAuth with right jwt', apiKey, ContentType.TEXT)
    fclient.setAuth(apiKey)

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('hello', {})

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  test('invoke hello with auth header in invoke', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`)

    log('invoke hello with Authorization header')
    const { data, error } = await fclient.invoke<string>('hello', {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  test('invoke hello with auth header override in invoke', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient with wrong jwt')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`)

    const wrongKey = sign({ name: 'anon' }, 'wrong_jwt')
    attach('setAuth with wrong jwt', wrongKey, ContentType.TEXT)
    fclient.setAuth(wrongKey)

    log('invoke hello with Authorization header')
    const { data, error } = await fclient.invoke<string>('hello', {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  // no jwt yet
  test.skip('invoke hello with wrong auth header overridden in invoke', async () => {
    /**
     * @feature auth
     */
    log('create FunctionsClient with wrong jwt')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`, {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })

    const wrongKey = sign({ name: 'anon' }, 'wrong_jwt')
    log('invoke hello with wrong Authorization header')
    const { data, error } = await fclient.invoke<string>('hello', {
      headers: {
        Authorization: `Bearer ${wrongKey}`,
      },
    })

    log('check error')
    expect(error).not.toBeNull()
    expect(error?.message).toEqual('Relay Error invoking the Edge Function')
    expect(data).toBeNull()
  })

  it.skip('invoke missing function', async () => {
    /**
     * @feature errors
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`, {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    })

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('missing', {})

    log('check error')
    expect(error).not.toBeNull()
    expect(error?.message).toEqual('Invalid JWT')
    expect(data).toBeNull()
  })

  test('invoke with custom fetch', async () => {
    /**
     * @feature fetch
     */
    log('create FunctionsClient')
    const fclient = new FunctionsClient(`http://localhost:${relay.container.getMappedPort(port)}`, {
      customFetch: getCustomFetch(
        `http://localhost:${relay.container.getMappedPort(port)}/${'hello'}`,
        {
          method: 'POST',
          headers: {
            Authorization: `Bearer ${apiKey}`,
          },
        }
      ),
    })

    log('invoke hello')
    const { data, error } = await fclient.invoke<string>('', {})

    log('assert no error')
    expect(error).toBeNull()
    log(`assert ${data} is equal to 'Hello World'`)
    expect(data).toEqual('Hello World')
  })

  // regression: https://github.com/supabase/edge-runtime/issues/50
  test('invoke preflight request', async () => {
    log('perform a preflight request')
    const res = await getCustomFetch(`http://localhost:${relay.container.getMappedPort(port)}/hello`, {
      method: 'OPTIONS',
      headers: {
        Authorization: `Bearer ${apiKey}`
      }
    })('')

    expect(await res.text()).toMatchInlineSnapshot()
  })
})
