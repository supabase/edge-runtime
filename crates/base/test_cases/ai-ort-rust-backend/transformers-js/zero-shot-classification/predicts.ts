const predictCommon = {
  sequence: "I love making pizza",
  labels: [ "cooking", "travel", "dancing" ],
}
export const predicts = {
  'arm64': {
    ...predictCommon,
    scores: [0.9991624363954653, 0.00047260279251281554, 0.00036496081202200547]
  },
  'x64': {
    ...predictCommon,
    scores:[0.9991624362472264, 0.0004726026797654259, 0.0003649610730082667]
  },
}
