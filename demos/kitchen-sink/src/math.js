function add(a, b) {
  return a + b;
}

function multiply(a, b) {
  return a * b;
}

function average(numbers) {
  const sum = numbers.reduce(add, 0);
  return sum / numbers.length;
}

module.exports = { add, multiply, average };
