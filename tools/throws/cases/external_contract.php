<?php
declare(strict_types=1);
namespace ThrowsProbe\external;

require_once __DIR__ . '/../dependencies/external.php';

function caller(): void { \ProbeDependency\leaf(); }
