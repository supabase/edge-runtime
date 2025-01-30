/// /// <reference path="./types.d.ts" />

/*
const modelUrl = 'https://huggingface.co/kalleby/hp-to-miles/resolve/main/model.onnx?download=true';
const modelConfigUrl =
  'https://huggingface.co/kalleby/hp-to-miles/resolve/main/config.json?download=true';

const model = await Supabase.ai.RawSession.fromUrl(modelUrl);
const modelConfig = await fetch(modelConfigUrl).then((r) => r.json());

Deno.serve(async (req: Request) => {
  const params = new URL(req.url).searchParams;
  const inputValue = parseInt(params.get('value'));

  const input = new Supabase.ai.RawTensor('float32', [inputValue], [1, 1]);
    .minMaxNormalize(modelConfig.input.min, modelConfig.input.max);

  const output = await model.run({
    'dense_dense1_input': input,
  });

  console.log('output', output);

  const outputTensor = output['dense_Dense4']
    .minMaxUnnormalize(modelConfig.label.min, modelConfig.label.max);

  return Response.json({ result: outputTensor.data });
});
*/

// transformers.js Compatible:
// import { Tensor } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.3.2';
//  const rawTensor = new Supabase.ai.RawTensor('string', urls, [urls.length]);
//  console.log('raw tensor', rawTensor );
//
//  const tensor = new Tensor(rawTensor);
//  console.log('hf tensor', tensor);
//
// 'hf tensor operations'
//  tensor.min(); tensor.max(); tensor.norm() ....

// const modelUrl =
//   'https://huggingface.co/pirocheto/phishing-url-detection/resolve/main/model.onnx?download=true';

/*
const { Tensor, RawSession } = Supabase.ai;

const model = await RawSession.fromHuggingFace('pirocheto/phishing-url-detection', {
  path: {
    template: `{REPO_ID}/resolve/{REVISION}/{MODEL_FILE}?donwload=true`,
    modelFile: 'model.onnx',
  },
});

console.log('session', model);

Deno.serve(async (_req: Request) => {
  const urls = [
    'https://clubedemilhagem.com/home.php',
    'http://www.medicalnewstoday.com/articles/188939.php',
    'https://magalu-crediarioluiza.com/Produto_20203/produto.php?sku=1',
  ];

  const inputs = new Tensor('string', urls, [urls.length]);
  console.log('tensor', inputs.data);

  const output = await model.run({ inputs });
  console.log(output);

  return Response.json({ result: output.probabilities });
});
*/

const { Tensor, RawSession } = Supabase.ai;

const session = await RawSession.fromHuggingFace('kallebysantos/vehicle-emission', {
  path: {
    modelFile: 'model.onnx',
  },
});

Deno.serve(async (_req: Request) => {
  // sample data could be a JSON request
  const carsBatchInput = [{
    'Model_Year': 2021,
    'Engine_Size': 2.9,
    'Cylinders': 6,
    'Fuel_Consumption_in_City': 13.9,
    'Fuel_Consumption_in_City_Hwy': 10.3,
    'Fuel_Consumption_comb': 12.3,
    'Smog_Level': 3,
  }, {
    'Model_Year': 2023,
    'Engine_Size': 2.4,
    'Cylinders': 4,
    'Fuel_Consumption_in_City': 9.9,
    'Fuel_Consumption_in_City_Hwy': 7.0,
    'Fuel_Consumption_comb': 8.6,
    'Smog_Level': 3,
  }];

  // Parsing objects to tensor input
  const inputTensors: Record<string, Supabase.Tensor<'float32'>> = {};
  session.inputs.forEach((inputKey) => {
    const values = carsBatchInput.map((item) => item[inputKey]);

    inputTensors[inputKey] = new Tensor('float32', values, [values.length, 1]);
  });

  const { emissions } = await session.run(inputTensors);
  console.log(emissions);
  // [ 289.01, 199.53]

  return Response.json({ result: emissions });
});
