function greet(name) {
  const greeting = "Hello, " + name + "!";
  console.log(greeting);
  return greeting;
}

function greetAll(names) {
  return names.map(greet);
}

module.exports = { greet, greetAll };
