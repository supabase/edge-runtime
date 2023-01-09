import { testFetch } from './module.js'

async function processEvent() {
  console.log('called process event');
  try {
      const age = await testFetch();
      console.log(age);

    //processEvent();
    }
  } catch (e) {
    console.log(e.toString());
  }
}

processEvent();
