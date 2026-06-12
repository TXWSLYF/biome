// should not generate diagnostics
const arr = [1, 2, 3];
const str = "hello world";

// already using includes
arr.includes(1);
str.includes("world");
!arr.includes(1);

// positional use of indexOf — result stored, not compared
const pos = arr.indexOf(1);

// indexOf with fromIndex argument — semantics differ, leave alone
arr.indexOf(1, 2) !== -1;
arr.indexOf(1, -1) !== -1;

// unrelated comparisons that happen to use indexOf result
arr.indexOf(1) > 0;
arr.indexOf(1) >= 1;
arr.indexOf(1) === 0;
arr.indexOf(1) < -1;

// indexOf result used in arithmetic
arr.indexOf(1) + 1;

// not a member call at all
indexOf(1) !== -1;

const posLast = arr.lastIndexOf(1);
arr.lastIndexOf(1, 2) !== -1;

const y = { x: 1 };

function isValid() {
    return true;
}

function getBar() {
    return 1;
}

// some() patterns that should not be flagged
arr.some(x => x == undefined);
arr.some(x => x !== 1);
arr.some((x, index) => x === index);
arr.some(x => (x === 1) && isValid());
arr.some(x => y === 1);
arr.some(x => y.x === 1);
arr.some(x => {
    const bar = getBar();
    return x === bar;
});
arr.some(x => x > 1);
