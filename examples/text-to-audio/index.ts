// Setup type definitions for built-in Supabase Runtime APIs
import 'jsr:@supabase/functions-js/edge-runtime.d.ts';
import { PreTrainedTokenizer } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.3.1';

// import 'phonemize' code from Kokoro.js repo
import { phonemize } from './phonemizer.js';

const { Tensor, RawSession } = Supabase.ai;

const STYLE_DIM = 256;
const SAMPLE_RATE = 24000;
const MODEL_ID = 'onnx-community/Kokoro-82M-ONNX';

// https://huggingface.co/onnx-community/Kokoro-82M-ONNX#samples
const ALLOWED_VOICES = [
  'af_bella',
  'af_nicole',
  'af_sarah',
  'af_sky',
  'am_adam',
  'am_michael',
  'bf_emma',
  'bf_isabella',
  'bm_george',
  'bm_lewis',
];

const session = await RawSession.fromHuggingFace(MODEL_ID);

Deno.serve(async (req) => {
  const params = new URL(req.url).searchParams;
  const text = params.get('text') ?? 'Hello from Supabase!';
  const voice = params.get('voice') ?? 'af_bella';

  if (!ALLOWED_VOICES.includes(voice)) {
    return Response.json({
      error: `invalid voice '${voice}'`,
      must_be_one_of: ALLOWED_VOICES,
    }, { status: 400 });
  }

  const tokenizer = await loadTokenizer();
  const language = voice.at(0); // 'a'merican | 'b'ritish
  const phonemes = await phonemize(text, language);
  const { input_ids } = tokenizer(phonemes, {
    truncation: true,
  });

  // Select voice style based on number of input tokens
  const num_tokens = Math.max(
    input_ids.dims.at(-1) - 2, // Without padding;
    0,
  );

  const voiceStyle = await loadVoiceStyle(voice, num_tokens);

  const { waveform } = await session.run({
    input_ids,
    style: voiceStyle,
    speed: new Tensor('float32', [1], [1]),
  });

  // Do `wave` encoding from rust backend
  const audio = await waveform.tryEncodeAudio(SAMPLE_RATE);

  return new Response(audio, {
    headers: {
      'Content-Type': 'audio/wav',
    },
  });
});

async function loadVoiceStyle(voice: string, num_tokens: number) {
  const voice_url =
    `https://huggingface.co/onnx-community/Kokoro-82M-ONNX/resolve/main/voices/${voice}.bin?download=true`;

  console.log('loading voice:', voice_url);

  const voiceBuffer = await fetch(voice_url).then(async (res) => await res.arrayBuffer());

  const offset = num_tokens * STYLE_DIM;
  const voiceData = new Float32Array(voiceBuffer).slice(
    offset,
    offset + STYLE_DIM,
  );

  return new Tensor('float32', voiceData, [1, STYLE_DIM]);
}

async function loadTokenizer() {
  // BUG: invalid 'h' not JSON. That's why we need to manually fetch the assets
  // const tokenizer = await AutoTokenizer.from_pretrained(MODEL_ID);

  const tokenizerData = await fetch(
    'https://huggingface.co/onnx-community/Kokoro-82M-ONNX/resolve/main/tokenizer.json?download=true',
  ).then(async (res) => await res.json());

  const tokenizerConfig = await fetch(
    'https://huggingface.co/onnx-community/Kokoro-82M-ONNX/resolve/main/tokenizer_config.json?download=true',
  ).then(async (res) => await res.json());

  return new PreTrainedTokenizer(tokenizerData, tokenizerConfig);
}
