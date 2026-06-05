/**
 * Sequential deploy demo: deploy services one at a time in dependency order.
 *
 * Uses the splitFirst + loop + branch pattern for sequential iteration.
 * Each service is fully deployed and verified before the next one starts.
 *
 * Contrast with `.iterate().map(deploy)` which would deploy all services
 * concurrently — violating dependency ordering.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  type Iterator,
  loop,
  pipe,
  constant,
  Iterator as Iter,
  identity,
  bindInput,
} from "@barnum/barnum/pipeline";
import { getServices, deployService, verifyService } from "./handlers/deploy";

console.error("=== Sequential deploy demo ===\n");

const services = Iter.fromArray<string>().call(getServices);

loop<Iterator<string>, null>((recur, done) =>
  identity<Iterator<string>>()
    .splitFirst()
    .branch({
      None: done.call(constant<null>(null)),

      Some: bindInput<[string, Iterator<string>], never>((pair) => {
        const [service, rest] = pair.split();
        const verified = verifyService.call(deployService.call(service));
        return pipe(verified.drop(), recur.call(rest));
      }),
    }),
)
  .call(services)
  .run();
