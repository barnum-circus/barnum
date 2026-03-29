var divide = require("./math").divide;

function getMagicNumber() {
  return divide(42, 7);
}

module.exports = { getMagicNumber };
