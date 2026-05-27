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
  runPipeline,
  pipe,
  loop,
  drop,
  constant,
  Iterator as Iter,
  identity,
  bindInput,
} from "@barnum/barnum/pipeline";
import { getServices, deployService, verifyService } from "./handlers/deploy";

console.error("=== Sequential deploy demo ===\n");

runPipeline(
  pipe(
    getServices,
    Iter.fromArray<string>(),

    loop<null, Iterator<string>>((recur, done) =>
      identity<Iterator<string>>()
        .splitFirst()
        .branch({
          None: constant<null>(null).then(done),

          Some: bindInput<[string, Iterator<string>], never>((pair) => {
            const [service, rest] = pair.split();
            return pipe(
              service,
              deployService,
              verifyService,
              drop,
              rest,
              recur,
            );
          }),
        }),
    ),
  ),
);
