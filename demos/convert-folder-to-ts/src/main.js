var getMagicNumber = require("./helpers").getMagicNumber;

function computeResult() {
  var value = getMagicNumber();
  return value + 1;
}

module.exports = { computeResult };
