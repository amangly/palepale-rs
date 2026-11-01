// Regenerate the embedded terminal pixels after changing data/palepale-logo.svg.
const fs = require('node:fs');
const path = require('node:path');
const sharp = require('sharp');

async function main() {
  const folder = path.join(__dirname, '..', 'data');
  const config = fs.readFileSync(path.join(folder, 'logo.toml'), 'utf8');
  const dimension = (name) => Number(config.match(new RegExp(`^${name} = (\\d+)$`, 'm'))[1]);
  const { data, info } = await sharp(path.join(folder, 'palepale-logo.svg'))
    .resize(dimension('width'), dimension('height'), { fit: 'fill' })
    .ensureAlpha()
    .raw()
    .toBuffer({ resolveWithObject: true });
  if (info.channels !== 4) throw new Error('Logo must render as RGBA.');
  fs.writeFileSync(path.join(folder, 'palepale-logo.rgba'), data);
  const threshold = dimension('alpha_threshold');
  const symbol = (name) => JSON.parse(config.match(new RegExp(`^${name} = (".*")$`, 'm'))[1]);
  const covered = (x, y) => data[(y * info.width + x) * 4 + 3] >= threshold;
  const rows = [];
  for (let y = 0; y < info.height; y += 2) {
    let row = '';
    for (let x = 0; x < info.width; x++) {
      const top = covered(x, y), bottom = covered(x, y + 1);
      row += symbol(top ? (bottom ? 'full' : 'upper') : (bottom ? 'lower' : 'empty'));
    }
    if (row.trim()) rows.push(row.trimEnd());
  }
  fs.writeFileSync(path.join(folder, 'palepale-logo-cells.txt'), rows.join('\n') + '\n');
  process.stdout.write(`Generated ${info.width}×${info.height} logo pixels.\n`);
}

main().catch((error) => {
  process.stderr.write(error.message + '\n');
  process.exitCode = 1;
});
