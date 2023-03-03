import { resolve } from 'path'
import { nanoid } from 'nanoid'
import crossFetch from 'cross-fetch'
import { sign } from 'jsonwebtoken'
import { GenericContainer, Network, StartedTestContainer, Wait } from 'testcontainers'
import { ExecResult } from 'testcontainers/dist/docker/types'
import { attach, log } from '../utils/jest-custom-reporter'
import { ContentType } from 'allure-js-commons'

/**
 * A Relay contains a running relay container that has a unique ID and two promises:
 * one for the `deno cache` and one for the `deno run` the required function
 */
export class Relay {
  container: StartedTestContainer
  id: string
  constructor(container: StartedTestContainer, id: string) {
    this.container = container
    this.id = id
  }
}

const port = 9000

/**
 * It starts a docker container with a edge-runtime, and waits for it to be ready
 * @param {string} slug - the name of the container
 * @param {string} path - the path of the functions directory to deploy
 * @param {string} importMap - the path of the import map ('/usr/services/{path/to/import_map.json}')
 * @param {Map<string, string>} env - list of environment variables for edge-runtime
 * @returns {Promise<Relay>} A Relay object.
 */
export async function runRelay({
  slug = 'edge-runtime',
  path = './functions',
  importMap = '/usr/services/import_map.json',
  env,
}: {
  slug?: string
  path?: string
  importMap?: string
  env?: Map<string, string>
}): Promise<Relay> {
  // read function to deploy
  log('read function body')
  const absPath = resolve(path)

  // random id for parallel execution
  const id = nanoid(5)

  //create network
  log('add network')
  const network = await new Network({ name: 'supabase_network_' + id }).start()

  // create relay container
  log(`create relay ${slug + '-' + id}`)
  const relay = await new GenericContainer('edge-runtime')
    .withName(slug + '-' + id)
    .withBindMount(absPath, '/usr/services', 'ro')
    .withNetworkMode(network.getName())
    .withExposedPorts(port)
    // .withWaitStrategy(Wait.forLogMessage('Listening on http://0.0.0.0:port'))
    .withStartupTimeout(15000)
    .withReuse()
    .withCmd(['start', '--dir', '/usr/services', '--import-map', importMap])

  // add envs
  env && env.forEach((value, key) => relay.withEnv(key, value))

  // start relay and function
  log(`start relay ${slug + '-' + id}`)
  const startedRelay = await relay.start()

  // wait till function is running
  log(`check function is healthy: ${slug + '-' + id}`)
  for (let ctr = 0; ctr < 30; ctr++) {
    try {
      const healthCheck = await crossFetch(
        `http://localhost:${startedRelay.getMappedPort(port)}/hello`,
        {
          method: 'POST',
        }
      )
      if (healthCheck.ok || healthCheck.status === 101) {
        log(`edge runtime started to serve: ${slug + '-' + id}`)
        return { container: startedRelay, id }
      }
    } catch {
      /* we actually don't care about errors here */
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }

  // if function hasn't started, stop container and throw
  log(`function failed to start: ${slug + '-' + id}`)
  startedRelay.stop()
  throw new Error("function didn't start correctly")
}

/**
 * If the JWT_SECRET and DENO_ORIGIN environment is not set, set it
 * @param env - The environment variables.
 * @param {string} jwtSecret - The JWT secret.
 * @param {string} denoOrigin - The origin of the Deno server.
 * @returns {Map<string, string>} - `env` variables map.
 */
function parseEnv(
  env: Map<string, string> | undefined | null,
  jwtSecret: string,
  denoOrigin: string
): Map<string, string> {
  if (env) {
    !env.has('JWT_SECRET') && jwtSecret && env.set('JWT_SECRET', jwtSecret)
    !env.has('DENO_ORIGIN') && denoOrigin && env.set('DENO_ORIGIN', denoOrigin)
  } else {
    env = new Map([
      ['JWT_SECRET', jwtSecret],
      ['DENO_ORIGIN', denoOrigin],
    ])
  }
  return env
}
