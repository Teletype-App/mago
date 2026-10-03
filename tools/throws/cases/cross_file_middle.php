<?php
declare(strict_types=1);
namespace ThrowsProbe\cross_file;

require_once __DIR__ . '/cross_file_leaf.php';

function middle(): void { leaf(); }
