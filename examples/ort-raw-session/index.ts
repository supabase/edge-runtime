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
  const inputTensors = {};
  session.inputs.forEach((inputKey) => {
    const values = carsBatchInput.map((item) => item[inputKey]);

    inputTensors[inputKey] = new Tensor('float32', values, [values.length, 1]);
  });

  const { emissions } = await session.run(inputTensors);
  // [ 289.01, 199.53]

  return Response.json({ result: emissions });
});
