const predictCommon = {
  token_str: 'london',
  sequence: 'london is the capital of england.',
}

export const predicts = {
  'arm64': {
    ...predictCommon,
    score: 0.3513390123844147,
  },
  'x64': {
    ...predictCommon,
    score: 0.3513388931751251
  },
}
