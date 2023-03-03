import type { Config } from '@jest/types'

const config: Config.InitialOptions = {
  preset: 'ts-jest',
  transform: {
    '^.+\\.ts?$': 'ts-jest',
  },
  moduleFileExtensions: ['ts', 'js', 'cjs', 'json', 'node'],
  setupFilesAfterEnv: ['./utils/jest-custom-reporter.ts'],
  testTimeout: 60000,
  collectCoverageFrom: ['!**/node_modules/**'],
  coverageProvider: 'v8',
  coverageThreshold: {
    global: {
      branches: 0,
      functions: 0,
      lines: 0,
      statements: 0,
    },
  },
}
export default config
