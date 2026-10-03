<?php
declare(strict_types=1);
namespace ProbeDependency;

/** @throws \LengthException */
function leaf(): void { throw new \LengthException(); }
