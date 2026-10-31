const label = n === 0 ? "none" : n === 1 ? "one" : n < 10 ? "a few" : "many";
let a = 1,
  b = 2;
[a, b] = [b, a];
if (a) b++;
else {
  b--;
}
